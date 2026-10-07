type Verdict = { version: 1; verdict: "allow" } | { version: 1; verdict: "deny"; message: string };

// Permission checks are read-only and repeatable. State invariants belong in the
// database write conditions, since this callback runs before the write transaction.
export function decide(value: unknown): Verdict {
  const denied: Verdict = { version: 1, verdict: "deny", message: "Permission denied" };
  if (!object(value) || value.version !== 1 || value.guard !== "caller_can_place" || value.callable !== "place_order") return denied;
  if (!object(value.ctx) || value.ctx.user !== "buyer" || typeof value.ctx.org !== "string") return denied;
  if (!object(value.args) || !Number.isSafeInteger(value.args.total) || (value.args.total as number) < 0) return denied;
  return { version: 1, verdict: "allow" };
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
