import assert from "node:assert/strict";
import { mkdtemp, cp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import { callback } from "./callback.js";
import { edge } from "./edge.js";
import { listen, close } from "./listener.js";
import { run, startBased, stop } from "./based-process.js";

async function post(url: string, body: unknown, headers: Record<string, string> = {}) {
  const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json", ...headers }, body: JSON.stringify(body), signal: AbortSignal.timeout(6000) });
  return { status: response.status, body: await response.json() };
}

async function demo() {
  const source = fileURLToPath(new URL("../", import.meta.url));
  const binary = process.env.BASED_BIN ? resolve(process.env.BASED_BIN) : "based";
  const directory = await mkdtemp(join(tmpdir(), "based-ts-guards-"));
  const secret = randomBytes(32).toString("hex");
  const policy = callback(secret);
  let based: Awaited<ReturnType<typeof startBased>> | undefined;
  let gateway: ReturnType<typeof edge> | undefined;
  try {
    await cp(join(source, "schema"), join(directory, "schema"), { recursive: true });
    await cp(join(source, "based.toml"), join(directory, "based.toml"));
    const callbackUrl = await listen(policy);
    await writeFile(join(directory, "guards.toml"), `[[guards]]\nname = "caller_can_place"\nendpoint = "${callbackUrl}/check"\nsecret_env = "DEMO_CALLBACK_SECRET"\nallow_loopback_http = true\n`);
    const env = { ...process.env, BASED_DATABASE_URL: join(directory, "demo.db"), BASED_IDEMPOTENCY_STORE: "memory", BASED_INIT_IDEMPOTENCY_TABLE: "false", DEMO_CALLBACK_SECRET: secret };
    await run(binary, ["migrate", "gen"], directory, env);
    await run(binary, ["migrate", "apply"], directory, env);
    based = await startBased(binary, directory, env);
    // Local setup only: create_org is unavailable through the user-facing gateway.
    const org = await post(`${based.url}/m/create_org`, { name: "Demo" });
    assert.equal(org.status, 200);
    gateway = edge(based.url, org.body.id);
    const publicUrl = await listen(gateway);
    assert.equal((await post(`${callbackUrl}/check`, { version: 1 }, { Authorization: "Bearer forged" })).status, 401);
    assert.equal((await post(`${publicUrl}/m/place_order`, { total: 7 })).status, 401);
    const buyer = { Authorization: "Bearer local-buyer-token", "Idempotency-Key": "demo-order" };
    const allowed = await post(`${publicUrl}/m/place_order`, { total: 7 }, buyer);
    assert.equal(allowed.status, 200);
    assert.deepEqual(await post(`${publicUrl}/m/place_order`, { total: 7 }, buyer), allowed);
    const denied = await post(`${publicUrl}/m/place_order`, { total: 7 }, {
      Authorization: "Bearer local-viewer-token", "X-Based-Context": JSON.stringify({ org: org.body.id, user: "buyer" }), "X-Based-Shard-Key": "forged"
    });
    assert.equal(denied.status, 403);
    assert.equal(denied.body.error.code, "guard_denied");
    assert.equal((await post(`${publicUrl}/m/create_org`, { name: "unauthorized" }, buyer)).status, 404);
    console.log("buyer: allowed (200); viewer with forged context: denied (403)");
    await close(policy);
    const unavailable = await post(`${publicUrl}/m/place_order`, { total: 7 }, buyer);
    assert.equal(unavailable.status, 403);
    assert.equal(unavailable.body.error.message, "Guard check unavailable");
    const orders = await post(`${publicUrl}/q/my_orders`, {}, buyer);
    assert.equal(orders.status, 200);
    assert.equal(orders.body.length, 1);
    console.log("forged callback: rejected (401); stopped callback: denied (403); persisted orders: 1");
  } finally {
    if (gateway) await close(gateway);
    await close(policy);
    if (based) await stop(based.process);
    await rm(directory, { recursive: true, force: true });
  }
}

await demo();
