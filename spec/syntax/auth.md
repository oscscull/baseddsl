# syntax/auth.md

Principles: 2 (nothing consequential true by omission — an important contract is **written,
not implied**), 4 (one source of truth; reference by name), 5 (enforcement is ours, decisions
aren't), 7 (lend the intent).

## Boundary
This layer does NOT make authz decisions (roles, policy, branching if/then) — that's caller logic,
over the line, Turing-complete, depends on context we don't have. We only enforce a constraint the
caller supplies, via the predicate-injection we already own (same mechanism as soft-delete). Three
handles; in all three the complex/branching logic lives in the caller's host language.

## Handle 1 — caller-set context, queries consume
Caller's auth code computes the decision and passes the result as context. Queries reference it as a param.
```
query my_orders() -> OrderCard[] {
  list Order where (org = $ctx.org);
}
```
Not a decision — a filter value the caller produced.

## Handle 2 — named scope (a written contract, referenced on both sides)
A **scope** is a standing filter, parameterized by request context, injected into every query **and
write** on a model — exactly like soft-delete. It is a contract important enough that it must be
*written, not implied*: declared once as a top-level `scope` decl, then referenced **by name** on
each side — the model it governs (`@scope Name`) and every callable that touches that model
(`scoped Name`). We enforce the constraint shape; the caller supplies the value. No branching.

### The `scope` declaration
Declared once, like a shape (D46). Each term is `col: Type = $ctx.field`:
```
scope Tenant (org: Org = $ctx.org)
```
Read: "the *Tenant* scope filters on column `org` (type `Org`), bound to `$ctx.org`."

- **The predicate keeps the exact restricted form** of the old inline `@scope` (D32): a conjunction
  of `col = $ctx.field` equalities, nothing else — no `or`, `in`, range, literal RHS, multi-hop path,
  or named filter (`E0180`). That restriction is what lets a scope be injected *everywhere* and, in
  particular, **auto-set on `create`**; a non-equality/multi-owner rule has no create-time projection
  and is not a scope (it is a Handle-1 filter, below). A conjunction is written comma-separated:
  `scope Region (org: Org = $ctx.org, region: Region = $ctx.region)`.
- **The scope decl is where `$ctx.<field>`'s type is declared** (D46). `org: Org` states the type of
  both the column each governed model must carry *and* the `$ctx.org` request field — they are one
  type by the equality, sourced once here (P4). This **ends the per-callable `$ctx` inference for the
  scope field** (D4/D5): a scoped callable reads `$ctx.org`'s type from `Tenant`, not from whichever
  column it happened to compare against. Coherence for the scope field is now structural — one decl,
  one type — so it can never clash across callables. (`$ctx` fields used only in hand-written Handle-1
  `where`s or `guard` args are still inferred per callable, D4.)

### The model reference — `@scope Name` (repeatable; a set of alternatives)
A model opts into a scope by naming it; the predicate is not restated (P4 — one source of truth):
```
@scope Tenant
Order {
  org:        Org          # the scope column — must exist, type must match the scope decl
  placed_by:  User
  total:      int
  ...
}
```
The governed model **must declare each named scope's column(s)** at a conforming type (`org: Org`) —
else `E0184` (checked per `@scope` decorator). A model whose *physical* column name differs aliases it
at the field (`org: Org (column "legacy_org")`, D3/D8); the field name still matches the scope. (A
per-model *field-name* override — `@scope Tenant(owner_org)` — is reserved but **deferred**; v1 requires
the field name to match the scope column name. See D46.)

**A model may declare `@scope` more than once (D47).** The stack of `@scope` decorators is a
**disjunction of conjunctions** (DNF) — each decorator is *one alternative* (a valid way to be scoped),
the commas *within* one decorator are a conjunction:
- `@scope Page, Author` — **one** alternative `{Page ∧ Author}`: a row/callable must confine by **both**.
- `@scope Page` + `@scope Author` (two stacked decorators) — **two** alternatives `{Page}`, `{Author}`:
  confining by **either** satisfies the contract.
- Mixed (`@scope Page, Author` + `@scope Admin`) — `(Page ∧ Author) ∨ Admin`. No new syntax; a comma is
  AND, a new line is OR.

Read: *each `@scope` decorator declares one valid confinement of this model.* Whichever alternative a
callable satisfies, the row is filtered by that alternative's axes — never left unfiltered, never a
*runtime* choice between them (that would be `guard`, below).

### The callable reference — `scoped Name` (the superset-of-an-alternative rule)
Any query/mutation whose target model is in a scope **must** acknowledge it — either accept the scope
(`scoped Name[, Name]*`) or opt out (`unscoped("reason")`). **Writing neither is a hard error
(`E0182`)**: the contract is too important to be true by omission (principle 2). `scoped …` sits where
`unscoped(...)` sits — after the return type on a query, after any `guard` on a mutation:
```
query order_by_id(id) -> OrderCard scoped Tenant;
query my_org_orders() -> OrderCard[] scoped Tenant { list Order; }

mutation place_order(buyer: Id, total: int) -> OrderCard scoped Tenant {
  create Order { placed_by = $buyer, total = $total };
}
```
`scoped Tenant` names the standing filter(s) injected; it does not restate the predicate (reference by
name, P4). It is the visible half of the both-sides contract: `@scope Tenant` on the model, `scoped
Tenant` on the callable.

**The uniform callable rule (D47).** A callable must confine by a set of scope axes that is a
**superset of at least one** of its target model's declared `@scope` alternatives — else
`unscoped("reason")`. In DNF terms: the `scoped …` set must satisfy ≥1 whole `@scope` decorator.
- AND model (`@scope Page, Author`): `scoped …` must include **both** `Page` and `Author` (the only
  alternative). Naming just one is too few axes → `E0185`.
- OR model (`@scope Page` + `@scope Author`): `scoped Page` **or** `scoped Author` each satisfies one
  alternative — either is enough.
- Naming *extra* / narrower axes (any superset of an alternative) is allowed and safe — more confinement
  never leaks. Naming an axis the model has no `@scope` for is `E0185`. Naming none requires `unscoped`.

This vindicates the "input ⊇ allowed scopes" intuition, now precise: the callable's confinement axes
must ⊇ one declared alternative.

What the acknowledgement means, per operation (unchanged from D32/D34):
- **Reads + writes:** each named axis's `col = $ctx.field` equality is ANDed into every `WHERE`
  (updates/deletes/restores can't touch an out-of-scope row) and into every *joined* scoped table's
  `ON` (a relation reach can't read across a scope boundary, D34). The injected predicate is the
  **conjunction of the named axes** — the alternative the callable chose.
- **Create:** the scope columns are **engine-managed** — auto-set from `$ctx`, never caller params. A
  create auto-sets every scope column whose `$ctx` field is available and **must satisfy ≥1 of the
  model's `@scope` alternatives** (all axes of at least one decorator get set), so no row is ever created
  unowned — closing the accidentally-unfiltered hole on the write side. Assigning a scope column is
  still `E0181`; a create that can satisfy **no** alternative — or a required non-null scope column whose
  `$ctx` field is absent at create — is `E0186`.

### Multi-scope callables (a set per touched model)
A query reaching a *second* scoped model through a relation (D34 joined-`ON`) is in **both** models'
scopes; the `scoped …` set must satisfy ≥1 alternative for **each** scoped model it touches (root plus
every scoped model reached), one `scoped` clause, comma-separated:
```
query ticket_with_contact(id) -> TicketCard scoped Tenant, Region;
```
A touched scoped model left unsatisfiable by the set (too few axes for any of its alternatives), or an
axis no touched model declares any `@scope` for, is `E0185`. This makes D34's joined-scope enforcement
*written*: the reader sees every scope boundary the query crosses.

### Worked example — a model with two alternatives (OR) and one with two axes (AND)
`Post` is confinable **either** by the page it belongs to **or** by its author — two independent,
each-sufficient ways to own a row. Two stacked `@scope` decorators (OR):
```
scope Page   (page:   Page = $ctx.page)
scope Author (author: User = $ctx.user)

@scope Page
@scope Author
Post {
  page:    Page
  author:  User
  body:    text
  ...
}

query posts_on_page() -> PostCard[] scoped Page   { list Post; }   # confined by page
query my_posts()      -> PostCard[] scoped Author { list Post; }   # confined by author
```
Each query names **one** alternative and is fully confined — never bare, never `E0182`. `posts_on_page`
injects `page = $ctx.page`; `my_posts` injects `author = $ctx.user`. A cross-cutting admin view opts out:
`query all_posts() -> PostCard[] unscoped("admin: moderation queue")`.

Contrast with an **AND** model — a comment that is owned only by page *and* author together:
```
@scope Page, Author
Comment { page: Page  author: User  body: text  ... }

query my_comments_on_page() -> CommentCard[] scoped Page, Author { list Comment; }
```
Here `scoped Page` alone is `E0185` (the single alternative needs both axes); the callable must name both,
and a `create Comment` must have both `$ctx.page` and `$ctx.user` present (else `E0186`).

### Escape hatch — `unscoped("reason")`
Cross-scope access (admin/support/jobs/import) opts one callable out of scope entirely — read + write
injection, the joined-`ON` injection, *and* the create auto-set — with a **mandatory reason** (never
silent, principle 6), greppable, and linted:
```
query orders_in_org(org) -> OrderCard[] unscoped("admin: cross-org order lookup");
```
It forfeits *only* scope — soft-delete still applies. `unscoped` and `scoped` are mutually exclusive
(a callable does one or the other). `W0106` flags a stale `unscoped` (target is in no scope).

### What the compiler guarantees / does not
It guarantees the scope predicate is injected everywhere — root `WHERE`, write-target `WHERE`, joined
`ON`, and the create auto-set — except explicit `unscoped` sites, and that cross-scope creates are
inexpressible; it kills accidental leaks and forces every crossing to be *named* in source. It does
**not** verify the predicate is the *right* rule, or evaluate any role matrix (that's Handle 3). A
scope is a row-visibility filter, not a checked authorization model.

### Not a scope
Not uniform (differs by operation) or multi-owner (`org in $ctx.orgs`)? That is not a scope — use a
per-query `where` (Handle 1). Real decisions → Handle 3.

**Not a runtime disjunction, either.** Multiple `@scope` alternatives are still *static* confinement:
each callable picks **one** alternative at author time (`scoped Page` *or* `scoped Author`), and the row
set differs by which it picked — `posts_on_page` and `my_posts` return different rows. A `WHERE page = …
OR author = …` decided **at request time** — where the returned data is identical and you are only
checking whether the caller holds *some* credential — is **not** a scope: it is a Handle-3 `guard`
(a decision, host-language, over the line, principle 5). Rule of thumb: if the disjunction changes *which
rows come back*, it is a set of `@scope` alternatives (each an un-forgettable filter); if it only gates
*whether the same rows come back*, it is a `guard`.

### Error set (E018x band, D46 → revised D47)
| Code | Triggers |
|------|----------|
| `E0180` | a `scope` decl's predicate isn't a conjunction of `col = $ctx.field` |
| `E0181` | a `create` assigns a scope column (engine-managed — cross-scope create is inexpressible) |
| `E0182` | a callable whose target is scoped writes *neither* `scoped …` *nor* `unscoped(…)` (the required-declaration rule) |
| `E0183` | `@scope Name` / `scoped Name` references a `scope` decl that doesn't exist |
| `E0184` | a `@scope` model lacks the scope's column, or declares it at a non-conforming type (per decorator) |
| `E0185` | a callable's `scoped …` set doesn't ⊇ any declared alternative of a touched scoped model (too few axes to satisfy any alternative), or names an axis no `@scope` on a touched model declares |
| `E0186` | a `create` can satisfy **no** alternative of the target model (a required non-null scope column has no `$ctx` value) |
| `W0106` | `unscoped(…)` on a callable whose target is in no scope (stale) |

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
