import { createServer } from "node:http";
import { authenticates } from "./auth.js";
import { readBody } from "./read-body.js";
import { reply } from "./reply.js";
import { decide } from "./policy.js";

export function callback(secret: string) {
  if (!secret.trim()) throw new Error("Callback secret required");
  return createServer(async (request, response) => {
    if (!authenticates(request.headers.authorization, secret)) {
      reply(response, 401, { error: "Unauthorized" });
      return;
    }
    if (request.method !== "POST" || request.url !== "/check") {
      reply(response, 404, { error: "Not found" });
      return;
    }
    try {
      const body = await readBody(request, 256 * 1024);
      reply(response, 200, decide(JSON.parse(body.toString("utf8"))));
    } catch {
      reply(response, 400, { error: "Invalid request" });
    }
  });
}
