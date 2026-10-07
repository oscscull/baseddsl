import { timingSafeEqual } from "node:crypto";

export function authenticates(header: string | undefined, secret: string): boolean {
  const expected = Buffer.from(`Bearer ${secret}`);
  const supplied = Buffer.from(header ?? "");
  return supplied.length === expected.length && timingSafeEqual(supplied, expected);
}
