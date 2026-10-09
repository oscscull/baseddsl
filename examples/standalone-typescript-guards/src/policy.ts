type Verdict = { version: 1; verdict: "allow" } | { version: 1; verdict: "deny"; message: string };

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Read-only and repeatable. Atomic invariants belong in database write conditions.
export function decide(value: unknown): Verdict {
  const denied: Verdict = { version: 1, verdict: "deny", message: "Permission denied" };
  if (!object(value) || value.version !== 1 || value.guard !== "caller_can_place" || value.callable !== "place_order") return denied;
  if (!object(value.ctx) || value.ctx.user !== "buyer" || typeof value.ctx.org !== "string") return denied;
  if (!object(value.args) || typeof value.args.total !== "number" || !Number.isSafeInteger(value.args.total) || value.args.total < 0) return denied;
  return { version: 1, verdict: "allow" };
}
