import type { Server } from "node:http";
import { once } from "node:events";

export async function listen(server: Server): Promise<string> {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Missing listener address");
  return `http://127.0.0.1:${address.port}`;
}

export async function close(server: Server): Promise<void> {
  if (!server.listening) return;
  server.close();
  server.closeAllConnections();
  await once(server, "close");
}
