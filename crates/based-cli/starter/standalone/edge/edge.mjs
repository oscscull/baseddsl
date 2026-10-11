// Example authentication edge: the only listener callers reach.
import { createServer } from "node:http";
import { context } from "./identity.mjs";

const based = process.env.BASED_URL ?? "http://127.0.0.1:9090";
const port = Number(process.env.EDGE_PORT ?? 8080);
const route = /^\/[qm]\/[A-Za-z_][A-Za-z0-9_]*$/;
const limit = 1024 * 1024;

createServer(async (request, response) => {
  const ctx = context(request.headers.authorization);
  if (!ctx) return reply(response, 401, { error: "unauthorized" });
  if (request.method !== "POST" || !route.test(request.url ?? "")) {
    return reply(response, 404, { error: "not found" });
  }
  const body = await read(request);
  if (!body) return reply(response, 413, { error: "body too large" });
  // Fresh headers: caller X-Based-* headers never reach Based.
  const headers = {
    "Content-Type": "application/json",
    "X-Based-Context": JSON.stringify(ctx),
  };
  const key = request.headers["idempotency-key"];
  if (typeof key === "string") headers["Idempotency-Key"] = key;
  try {
    const upstream = await fetch(based + request.url, {
      method: "POST",
      headers,
      body,
      redirect: "error",
      signal: AbortSignal.timeout(5000),
    });
    reply(response, upstream.status, await upstream.text());
  } catch {
    reply(response, 502, { error: "based unavailable" });
  }
}).listen(port, "127.0.0.1", () => {
  console.log(`edge listening on http://127.0.0.1:${port}, forwarding to ${based}`);
});

async function read(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > limit) return undefined;
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

function reply(response, status, body) {
  response.writeHead(status, { "Content-Type": "application/json" });
  response.end(`${typeof body === "string" ? body : JSON.stringify(body)}\n`);
}
