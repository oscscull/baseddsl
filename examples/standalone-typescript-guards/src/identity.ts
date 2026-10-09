import { authenticates } from "./auth.js";

export type Context = { org: string; user: "buyer" | "viewer" };

// Local-only credentials. Replace with verified sessions and authorized membership.
export function identity(header: string | undefined, org: string): Context | undefined {
  if (authenticates(header, "local-buyer-token")) return { org, user: "buyer" };
  if (authenticates(header, "local-viewer-token")) return { org, user: "viewer" };
  return undefined;
}
