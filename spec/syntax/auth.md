# syntax/auth.md

Principles: 5 (enforcement is ours, decisions aren't), 7 (lend the intent).

## Boundary
This layer does NOT make authz decisions (roles, policy, branching if/then) — that's caller logic, over the line, Turing-complete, depends on context we don't have. We only enforce a constraint the caller supplies, via the predicate-injection we already own (same mechanism as soft-delete). Three handles; in all three the complex/branching logic lives in the caller's host language.

## Handle 1 — caller-set context, queries consume
Caller's auth code computes the decision and passes the result as context. Queries reference it as a param.
```
query my_orders() -> OrderCard[] {
  list Order where (org = $ctx.org);
}
```
Not a decision — a filter value the caller produced.

## Handle 2 — model scope predicate (auto-injected)
A standing filter, parameterized by context, injected into every query **and write** on the model — exactly like soft-delete. Uniform, single-owner (D32).
```
@scope(org = $ctx.org)
Order { ... }
```
We enforce the constraint shape; caller supplies the value. No branching.

Restricted to a conjunction of `col = $ctx.field` equalities (D32) — that is what makes it *uniform* and lets it be enforced on every operation:
- **Reads + writes:** the predicate is ANDed into every `WHERE` (updates/deletes/restores can't touch an out-of-scope row).
- **Create:** the scope column is **engine-managed** — auto-set from `$ctx`, never a caller param. A caller can't plant a row outside their own scope; a cross-scope `create` is *inexpressible* (assigning the column is an error). So `create Order { total = $t }` gets `org` from context automatically.
- Not uniform (differs by op) or multi-owner (`org in $ctx.orgs`)? That's not scope — use a per-query `where` (Handle 1). Real decisions → Handle 3.

**Escape hatch — `unscoped("reason")`.** Cross-scope access (admin/support/jobs/import) opts one callable out of scope entirely (read + write injection *and* the create auto-set), with a **mandatory reason** (never silent), greppable, and linted:
```
query orders_in_org(org) -> OrderCard[] unscoped("admin: cross-org order lookup");
```
It forfeits *only* `@scope` — soft-delete still applies.

**What the compiler guarantees / does not.** It guarantees the predicate is injected everywhere except explicit `unscoped` sites, and that cross-scope creates are inexpressible — killing accidental leaks. It does **not** verify the predicate is the *right* rule, or evaluate any role matrix (that's Handle 3). `@scope` is a row-visibility filter, not a checked authorization model.

## Handle 3 — guard hook into caller code
For real decisions, we don't evaluate — we invoke the caller's host-language fn at the boundary and respect its verdict.
```
query refund(order: Id, amount: int) -> RefundResult
  guard caller_can_refund
{ ... }
```
We own that the check runs; they own what it decides.

## Net
Simple "scope to caller's org" -> Handles 1+2, zero logic. Complex permissions -> caller's code + Handle 3. Never a policy engine.
