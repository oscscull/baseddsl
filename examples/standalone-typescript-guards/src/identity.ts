import { authenticates } from "./auth.js";

export interface Identity { org: string; user: string }

// A local demonstration of server-side credentials → identity. Real apps replace
// this with verified sessions and their own authorization of tenant membership.
export function identity(header: string | undefined, org: string): Identity | undefined {
  if (authenticates(header, "local-buyer-token")) return { org, user: "buyer" };
  if (authenticates(header, "local-viewer-token")) return { org, user: "viewer" };
  return undefined;
}
