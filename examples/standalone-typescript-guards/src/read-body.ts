import type { IncomingMessage } from "node:http";

export async function readBody(request: IncomingMessage, limit: number): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of request) {
    const bytes = Buffer.from(chunk);
    size += bytes.length;
    if (size > limit) throw new Error("Body too large");
    chunks.push(bytes);
  }
  return Buffer.concat(chunks);
}
