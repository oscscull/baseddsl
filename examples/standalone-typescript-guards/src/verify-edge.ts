import assert from "node:assert/strict";
import { createServer, type IncomingHttpHeaders } from "node:http";
import { edge } from "./edge.js";
import { listen, close } from "./listener.js";
import { post } from "./request.js";
import { reply } from "./reply.js";

export async function verifyEdgeHeaders(): Promise<void> {
  let received: IncomingHttpHeaders | undefined;
  const upstream = createServer((request, response) => {
    received = request.headers;
    reply(response, 200, []);
  });
  const backend = await listen(upstream);
  const gateway = edge(backend, "org-from-session");
  try {
    const url = await listen(gateway);
    const result = await post(`${url}/q/my_orders`, {}, {
      Authorization: "Bearer local-buyer-token",
      "X-Based-Context": JSON.stringify({ org: "forged-org", user: "viewer" }),
      "X-Based-Shard-Key": "forged-shard",
      "X-Based-Verdict": "allow",
      "Idempotency-Key": "client-key"
    });
    assert.equal(result.status, 200);
    assert.ok(received);
    assert.deepEqual(Object.keys(received).filter(name => name.startsWith("x-based-")).sort(), ["x-based-context"]);
    assert.deepEqual(JSON.parse(received["x-based-context"] as string), { org: "org-from-session", user: "buyer" });
    assert.equal(received.authorization, undefined);
    assert.equal(received["idempotency-key"], "client-key");
  } finally {
    await close(gateway);
    await close(upstream);
  }
}
