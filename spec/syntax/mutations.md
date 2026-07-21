# syntax/mutations.md

Principles: 1, 6, 7 (engine owns tx boundary).

## Same shape as queries, body writes
Named, typed, callable. Generates a client method + endpoint (calling.md).
```
mutation place_order(org: Id, buyer: Id) -> OrderCard {
  create Order { org = $org, placed_by = $buyer };
}
```

## Actions
- `create Model { field = $in, ... }`
- `update Model where (...) { field = $in }`
- `delete Model where (...)` — on a soft-delete model, rewritten to the soft action, never real DELETE.
- `restore Model where (...)`
- `hard delete Model where (...)` — explicit, loud opt-out for real DELETE. A real DELETE
  leaves no row to read back, so the mutation returns `ok` (below).

## Atomic update expressions
An `update` assignment's right-hand side may be a scalar **arithmetic expression** over the target
model's own numeric columns, params, and numeric literals — computed in the database as part of the
write, never read-modify-write:
```
mutation adjust_stock(id: Id, delta: int) -> ProductRow scoped Tenant {
  update Product where (id = $id) { qty = qty + $delta };
}
```
lowers to `SET qty = (qty + ?)` — one statement, no prior read, so concurrent adjustments compose
correctly (no lost update). Operators are `+ - * /`; `*` and `/` bind tighter than `+` and `-`,
left-associative, parenthesize to override. Operands are the updated model's numeric columns (a bare
name is the pre-write value of the row being touched), `$param`s, and numeric literals. Division is the
database's (integer vs. real per the operand types); the engine adds no zero-guard.

Eligibility is the numeric family (`int`, `float`, `decimal`) end to end: every column operand must be
numeric (`E0231`) and the assigned column must be numeric (`E0153`, the ordinary assign-type rule). An
arithmetic RHS is **update-only** — a `create` has no existing row to reference (`E0230`); a plain
value stays the one form for `create`. This is a leaf-level escape into arithmetic, not a general
language — no functions, no cross-row references, no conditionals (principle 5).

## Upsert (`create … on conflict update`)
A `create` may name a **conflict target** — a unique key — and an `update` branch that runs
instead of the insert when a row with that key already exists:
```
mutation record_hit(path: text) -> PageRow {
  create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
}
```
On the insert path a new row lands; on the conflict path the existing row's `update` branch
runs. The branch is an ordinary `update` assign block — plain values and the same
self-referential **arithmetic** an `update` allows (mutations.md above), so `hits = hits + 1`
composes on the **stored** value in the database (not a read-modify-write), the canonical
counter/accumulate use. The winning row is read back in the declared shape (below), keyed on
the **conflict target's value**, so the same shape decodes on both paths.

- **The conflict target must be a declared unique key** (`E0250`): a `(unique)` column, a
  `@index (…) unique` whose columns are exactly the named set, or the pk. Naming a
  non-unique column is the error — a conflict can only be defined against a key the database
  enforces.
- **Every conflict column must be set by the create** (`E0252`) — assigned in the block, or
  engine-managed as a `@scope` column — so the conflict, and the read-back key, have a value.
- **The `update` branch may not assign a conflict column** (`E0251`): moving the key would
  break the read-back and defeat the conflict.
- **A `@scope`d model's conflict target must include its scope column(s)** (`E0254`): else a
  conflict could match — and the `update` silently modify — a *different* scope's row. With
  the scope column in the key a conflict can only occur within the caller's own scope. (An
  `unscoped` mutation forfeits this, like every other scope guarantee.)
- **`on conflict` is not allowed on a `@soft_delete` model** (`E0253`): a tombstoned row still
  occupies its unique key, so an upsert would silently update the tombstone instead of
  inserting — surprising and unsafe. Delete-aware upsert is a separate, explicit feature.

Lowering is per-dialect over the `Dialect` seam: Postgres/SQLite `INSERT … ON CONFLICT (cols)
DO UPDATE SET …`, MariaDB `INSERT … ON DUPLICATE KEY UPDATE …` (its form carries no explicit
target list — the uniqueness of the validated key is what makes the two agree). `@scope`
auto-set and the read-back's scope/live guards apply exactly as on a plain `create`.

## Atomic groups
`tx { ... }` runs a static set of writes in one transaction; rolls back together. Back-reference a prior step with `^`:
```
tx {
  create User { email = $email };
  create Address { user = ^.id, city = $city };
}
```

## Scope acknowledgement (`scoped` / `unscoped`)
A mutation whose target model is in a scope **must** acknowledge it (auth.md Handle 2 / D46), exactly
like a query — `scoped Name` (accept) or `unscoped("reason")` (opt out), else `E0182`. The clause sits
after any `guard`, before the body. A model with several `@scope` alternatives (OR, D47) is satisfied by
naming **one**. On a scoped `create` the scope columns are engine-managed (auto-set from `$ctx`, never a
param — assigning one is `E0181`); the create **must satisfy ≥1 alternative** (all axes of some `@scope`
set, so no row lands unowned), else `E0186`:
```
mutation place_order(buyer: Id, total: int) -> OrderCard scoped Tenant {
  create Order { placed_by = $buyer, total = $total };   # `org` auto-set from $ctx
}
```

## Return shape (read-back)
A mutation's `-> Shape` is the row it wrote, read back in that shape after the write, inside the
same transaction (read-your-writes). A `create` reads back the created row; an `update` / soft
`delete` / `restore` reads back the row it touched (an update sees the new values). The read-back
projects the declared shape exactly as a `get` would — nested sub-objects and arrays included — and
applies the same scope/soft-delete guards. When the read-back finds **no row** — the `where` (with
those guards) matched nothing: a wrong id, or an id another scope owns — the mutation fails with
`not_found` (`404`) and the whole transaction rolls back, so nothing in the body survives the miss;
the caller gets a typed error, never an empty success. The response is identical whether the row is
absent or out of scope, so existence never leaks across a scope boundary.
(Implementation: D12 + D58 + D92.)

## Acknowledgement (`-> ok`) — destructive mutations
A **real DELETE** (a plain-model `delete` or `hard delete`) removes the row, so there is no
surviving row to read back — a declared shape could never decode. Such a mutation returns the bare
acknowledgement instead:
```
mutation purge_comment(id: Id) -> ok scoped Tenant {
  hard delete Comment where (id = $id);
}
```
The wire success is `{}`; the generated client method returns unit (`Result<(), ClientError>`);
OpenAPI advertises the shared empty `Ack` schema. A DELETE that matches **no row** — wrong id, or an
id another scope owns — is the same `not_found` (`404`) rollback as a surviving write's empty
read-back, with the same no-existence-leak response.

The two forms never mix (one way to say each thing):
- A shape on a mutation whose only write(s) on the return model are real DELETEs is an error
  (`E0220`) — declare `-> ok`.
- `-> ok` on a mutation with any surviving write (`create` / `update` / `restore` / soft `delete`)
  is an error (`E0221`) — a surviving write's read-back is the contract; declare its shape. A raw
  write may ride along (its effect is outside the engine's knowledge), but at least one real DELETE
  is required; the *first* real DELETE's model is the mutation's primary model (scope, sharding, and
  the 404 check ride on it).
- `-> ok` on a query is an error (`E0222`) — a query returns data.

## Read-decide-write
Not in the DSL. Use the host-language `transaction(closure)` seam: engine owns the boundary (commit on Ok, rollback on Err/panic, always release); caller writes logic; inside, queries are the same safe queries bound to the tx. See architecture docs.
