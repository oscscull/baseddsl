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
- `create Model { field = $in, ... }` — inline assign block (computed/param values)
- `create Model from $row` / `create Model[] from $rows` — structured shape-input insert (below)
- `update Model where (...) { field = $in }`
- `delete Model where (...)` — on a soft-delete model, rewritten to the soft action, never real DELETE.
- `delete all Model` — whole-table wipe (below). `all` is required and greppable.
- `restore Model where (...)`
- `hard delete Model where (...)` — explicit, loud opt-out for real DELETE. A real DELETE
  leaves no row to read back, so the mutation returns `ok` (below).
- `hard delete all Model` — whole-table wipe, physically (below).

## Whole-table wipe (`delete all`)
`delete all Model` / `hard delete all Model` deletes **every row** — the bulk counterpart of a
filtered delete. The `all` keyword is **required and greppable**: a bare `delete Model` (no `where`,
no `all`) is a parse error, so "delete everything" is a deliberate, visible choice, never a forgotten
filter (principle 1).
```
mutation clear_cache() -> ok {
  hard delete all CacheEntry;
}
```
- **Contract = delete-semantics.** Every row is gone, atomically inside the surrounding transaction.
- **Scope still applies.** On a `@scope`d model, `all` means every row *in the caller's scope* — the
  scope predicate rides the wipe, so another tenant's rows survive. (`unscoped` forfeits this.)
- **Soft-delete model.** `delete all` **tombstones every live row** (one `UPDATE … SET <tombstone>`
  with no filter beyond the live + scope guards), never a real DELETE. `hard delete all` still
  physically removes rows (the loud opt-out).
- **Read-back is `-> ok`.** A wipe has no single surviving row to read back, so it acknowledges
  (below) — `-> ok` is required; a declared shape is `E0220`. Unlike a filtered delete, a wipe of an
  **already-empty** table is a success, not a 404 (there is no specific row that had to exist).
- **Lowering (per-dialect over the `Dialect` seam).** An unfiltered `hard delete all` (no scope guard)
  lowers to `TRUNCATE` on **Postgres** (transactional there — rolls back with the surrounding tx) and
  to `DELETE FROM t` on **MySQL/MariaDB** (`TRUNCATE` is DDL and auto-commits — it would silently
  break the transaction) and **SQLite** (no `TRUNCATE` statement; an unfiltered `DELETE FROM t` already
  triggers its truncate optimization). A *scoped* wipe keeps its scope predicate, so it stays a
  `DELETE FROM t WHERE <scope>` on every dialect.

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

## Bulk / structured insert (`create … from`)

A `create` can take its column values from a **shape-typed param** instead of an inline
assign block — a `shape` doubles as the row-input type. `create Model[] from $rows`
inserts many rows (`$rows: SomeShape[]`) as one chunked, atomic multi-row INSERT;
`create Model from $row` inserts one (`$row: SomeShape`). The inline `create Model { … }`
form stays for computed/param assigns — this is the structured-record path.
```
shape ProductIn from Product { sku, name, price, category { id } }

mutation import_products(rows: ProductIn[]) -> ok scoped Tenant {
  create Product[] from $rows;
}
```
- **The shape is the input type.** No separate `input` decl — any shape a query can *read*
  can bulk-*write* back, with the same struct, zero transformation (the round-trip north
  star). Input-eligibility is checked at the **use site** (where the shape meets
  `create Model`), not on the shape decl:
  - every named scalar field maps to a settable column of `Model` (else `E0326`/`E0328`);
  - **required-column coverage** — every required column (NOT NULL, no default, not
    engine-managed) must be named by the shape, else `E0330`;
  - no computed / aggregate / raw field (`E0327`), no cross-relation reach (`E0328`);
  - the shape's `from` model must be the create target, and the param's arity must match
    (`shape[]` for `Model[]`, `shape` for `Model`) — else `E0325`.
- **Presence-driven columns.** A column **named** in the shape is written **verbatim** from
  the payload — including `id`, `@created`, `@updated`. A column **absent** is engine-filled:
  `id` is minted (`uuid`/`ulid`) or DB-generated (`serial`, omitted from the INSERT);
  `@created`/`@updated` stamp `now()`. **`@scope` is the sole exception:** it is *always*
  injected from `$ctx`, never the payload, even when the shape names it (tenant safety).
  Naming an `@updated`/`@created` column is legal but **warned** (`W0112`) — the explicit
  value overrides the auto-stamp; a named `@scope` value is silently overridden (`W0113`).
- **Relations = inline nested key blocks, one direction: FK link.** `category { id }` names
  exactly the target's key → sets the FK column(s) from the payload (round-trips: the same
  `{ id }` an output projection yields). A composite-key target names its key parts
  (`enrollment { student, course }`). A relation block naming **non-key payload** would
  create the related row too — a **nested write**, reserved and not yet supported (`E0329`).
  A bare relation, a named-shape nest, or a flatten as input is `E0328`.
- **Chunked + atomic.** The engine emits one `INSERT … VALUES (…),(…),…`, transparently
  chunked above the driver's bind limit (Postgres ~65535 binds — the user never sees the
  cap). The whole insert is all-or-nothing within the surrounding transaction.
- **Read-back is `-> ok`** in this release (`E0332` otherwise); a per-row `-> Shape[]` /
  single `-> Shape` read-back is a clean follow-on. The rows are verifiable with a query.
- **`on conflict` (bulk upsert)** on a `create … from` is not yet supported (`E0331`, BW2).

## Atomic groups
`tx { ... }` runs a static set of writes in one transaction; rolls back together. Bind a
step's produced row with `create … as <name>`, and reference a column of it from **any
later step** as `$name.field`:
```
tx {
  create User { email = $email } as user;
  create Address { user = $user.id, city = $city };
  create Log { actor = $user.id };            # reaches the first step
}
```
`$` unifies to "a value bound in this callable" — a param, a `$ctx` field, or a step
binding. A binding is **single-assignment** and **field-access only** (`$user.id`, never a
deeper traversal or a rebinding), so nothing Turing-complete enters the DSL (principle 5).
The overwhelming use is `$name.id`, wiring a just-created row's key into a later write.

**A bound create re-selects its written row (read-your-writes within the transaction).**
`create … as name` is the signal that the insert's result is needed, so the engine reads
the row the database actually wrote back immediately after the INSERT, and every
`$name.field` in a later step resolves to that row's **real committed value** — including a
value the create never named: an engine `@created`/`@updated` timestamp
(`CURRENT_TIMESTAMP`), a DB-side column default, or a DB-generated `serial` id. So a
sibling `at = $t.created_at` persists the ticket's actual `created_at`, and a `serial`
parent's `$t.id` wires its DB-assigned key into a child. The re-select folds into the
INSERT as `RETURNING <cols>` on Postgres/SQLite/MariaDB, and is a follow-up keyed `SELECT`
on MySQL (no `INSERT … RETURNING`). An **unbound** create does no such re-select.

- **`as <name>` is a keyword** — the bare trailing form (`create … user;`) would be two
  adjacent bare tokens, banned by principle 3.
- A binding name that **shadows a param**, or **duplicates** another binding in the same
  `tx`, is `E0280` — `$name` must name one thing.
- A `$name` that names **no param and no *prior* step binding** — an unknown name, or a
  **forward reference** to a binding declared later — is `E0281`; a binding reaches only
  the steps before it. `$name.field` where `field` isn't a column of the bound step's
  model reuses `unknown_field` (`E0111`).

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

**`-> ok` is the universal opt-out of read-back** (broadened in BW1 from real-DELETE-only): *any*
mutation may forfeit its declared-shape return with `-> ok` — a bulk `create Model[] from $rows -> ok`
is the motivating case (a large load skips echoing every row back). The primary model (scope, sharding)
is the first engine-known write's. The **zero-row 404** still fires only for a filtered **real DELETE**
(`hard delete M where …` / a plain-model `delete M where …`) affecting no row; a `create` / `update` /
`restore` / wipe under `-> ok` never 404s (opting out of read-back opts out of the not-found signal too).

The remaining rules (one way to say each thing):
- A shape on a mutation whose only write(s) on the return model are real DELETEs is an error
  (`E0220`) — declare `-> ok`.
- `-> ok` on a mutation with **no engine-known write** (a raw-only body — nothing to hang scope /
  sharding on) is an error (`E0221`). A raw write may ride along a real write, but cannot stand alone.
- A **whole-table wipe** (`delete all` / `hard delete all`, including a soft model's tombstone-all)
  is destructive — no single row survives — so `-> ok` is required on a wipe and a shape is `E0220`.
- `-> ok` on a query is an error (`E0222`) — a query returns data.

## Read-decide-write
Not in the DSL. Use the host-language `transaction(closure)` seam: engine owns the boundary (commit on Ok, rollback on Err/panic, always release); caller writes logic; inside, queries are the same safe queries bound to the tx. Full design — the three rungs (managed closure, explicit handle, BYO `adopt`), isolation levels, and `for update` locking — in **syntax/transactions.md** (D118).
