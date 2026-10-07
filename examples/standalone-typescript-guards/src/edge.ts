import { createServer } from "node:http";
import { identity } from "./identity.js";
import { readBody, reply } from "./http.js";

// This server is the only user-facing entry. Based and the callback bind loopback.
// Construct fresh headers: caller-supplied context/shard/verdict headers are ignored.
export function edge(based: string, org: string) {
  return createServer(async (request, response) => {
    const ctx = identity(request.headers.authorization, org);
    if (!ctx) { reply(response, 401, { error: "Unauthorized" }); return; }
    if (request.method !== "POST" || !["/m/place_order", "/q/my_orders"].includes(request.url ?? "")) {
      reply(response, 404, { error: "Not found" }); return;
    }
    try {
      const body = await readBody(request, 256 * 1024);
      const headers: Record<string, string> = { "Content-Type": "application/json", "X-Based-Context": JSON.stringify(ctx) };
      const key = request.headers["idempotency-key"];
      if (typeof key === "string") headers["Idempotency-Key"] = key;
      const upstream = await fetch(`${based}${request.url}`, { method: "POST", headers, body: body.toString("utf8"), redirect: "error", signal: AbortSignal.timeout(5000) });
      reply(response, upstream.status, await upstream.json());
    } catch { reply(response, 503, { error: "Service unavailable" }); }
  });
}
