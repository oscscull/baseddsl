// Local stand-in for your identity provider: bearer token -> trusted Based context.
import { timingSafeEqual } from "node:crypto";

const users = [
  ["alice-local-token", { owner: "00000000-0000-4000-8000-00000000000a" }],
  ["bob-local-token", { owner: "00000000-0000-4000-8000-00000000000b" }],
];

export function context(authorization) {
  const supplied = Buffer.from(authorization ?? "");
  for (const [token, ctx] of users) {
    const expected = Buffer.from(`Bearer ${token}`);
    if (supplied.length === expected.length && timingSafeEqual(supplied, expected)) {
      return ctx;
    }
  }
  return undefined;
}
