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
- `hard delete Model where (...)` — explicit, loud opt-out for real DELETE.

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
applies the same scope/soft-delete guards, so a row that lands or lives out of scope reads back as
absent. A **real DELETE** (a plain-model `delete` or `hard delete`) removes the row, so there is no
surviving row to return: those mutations return `{}` even if they declare a shape. (Implementation:
D12 + D58.)

## Read-decide-write
Not in the DSL. Use the host-language `transaction(closure)` seam: engine owns the boundary (commit on Ok, rollback on Err/panic, always release); caller writes logic; inside, queries are the same safe queries bound to the tx. See architecture docs.
