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
A standing filter, parameterized by context, injected into every query on the model — exactly like soft-delete (inherits soft-delete's cross-join correctness for free).
```
@scope(org = $ctx.org)
Order { ... }
```
We enforce the constraint shape; caller supplies the value. No branching.

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
