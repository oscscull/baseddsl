import assert from "node:assert/strict";
import { mkdtemp, cp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import type { Server } from "node:http";
import { callback } from "./callback.js";
import { edge } from "./edge.js";
import { listen, close } from "./listener.js";
import { run, startBased, stop } from "./based-process.js";
import { post } from "./request.js";
import { verifyPermissions, verifyUnavailable } from "./verify-guards.js";

async function demo() {
  const source = fileURLToPath(new URL("../", import.meta.url));
  const binary = process.env.BASED_BIN ? resolve(process.env.BASED_BIN) : "based";
  const directory = await mkdtemp(join(tmpdir(), "based-ts-guards-"));
  const secret = randomBytes(32).toString("hex");
  const policy = callback(secret);
  let based: Awaited<ReturnType<typeof startBased>> | undefined;
  let gateway: Server | undefined;
  try {
    await cp(join(source, "schema"), join(directory, "schema"), { recursive: true });
    await cp(join(source, "based.toml"), join(directory, "based.toml"));
    const callbackUrl = await listen(policy);
    await writeFile(join(directory, "guards.toml"), `[guards.caller_can_place]\nendpoint = "${callbackUrl}/check"\nsecret_env = "DEMO_CALLBACK_SECRET"\n`);
    const env = { ...process.env, BASED_DATABASE_URL: join(directory, "demo.db"), BASED_IDEMPOTENCY_STORE: "memory", BASED_INIT_IDEMPOTENCY_TABLE: "false", DEMO_CALLBACK_SECRET: secret };
    await run(binary, ["migrate", "gen"], directory, env);
    await run(binary, ["migrate", "apply"], directory, env);
    based = await startBased(binary, directory, env);
    // Local fixture setup; create_org is deliberately unavailable through the gateway.
    const org = await post(`${based.url}/m/create_org`, { name: "Demo" });
    assert.equal(org.status, 200);
    gateway = edge(based.url, org.body.id);
    const url = await listen(gateway);
    await verifyPermissions(url, org.body.id, callbackUrl);
    await close(policy);
    await verifyUnavailable(url);
  } finally {
    if (gateway) await close(gateway);
    await close(policy);
    if (based) await stop(based.process);
    await rm(directory, { recursive: true, force: true });
  }
}
await demo();
