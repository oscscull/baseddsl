# based — language reference

Every feature of `.bsl`, with the syntax to use it. One page; skim the index, jump to a section.
For *why* the language is shaped this way, see [`spec/`](../spec/); this page is *how*.

## Index

- [Project](#project) — files, `based.toml`, no implicit fields
- [Models & fields](#models--fields) — types, `?`, modifiers, `Id`, table naming, generated columns
- [Decorators](#decorators) — `@scope` `@index` `@created` `@soft_delete` `@fk` `@was` `@key` `@table`
- [Enums](#enums) — string & numeric
- [Relations](#relations) — to-one, to-many, inverse, custom join, composite, m2m
- [Indexes](#indexes) — single, composite, unique, method, raw
- [Soft delete](#soft-delete) — `delete` / `restore` / `hard delete`
- [Shapes](#shapes) — projections: nest, flatten, spread, computed, `case`, aggregates
- [Queries](#queries) — `get` / `list`, filters, params, `scoped`
- [Sorting](#sorting) · [Pagination](#pagination) · [Streaming](#streaming) · [Locking & distinct](#locking--distinct)
- [Named filters](#named-filters) · [Group by](#group-by)
- [Mutations](#mutations) — `create` / `update` / `delete` / `restore` / `tx`
- [Bulk & nested writes](#bulk--nested-writes) — `create M[] from $rows`
- [Upsert](#upsert) — `on conflict … update`, `incoming`, `...incoming`
- [Auth & scope](#auth--scope) — `$ctx`, `scope`, `guard`
- [Raw](#raw) — the escape hatch
- [Migrations](#migrations) · [Client](#client)

---

## Project

- Source is one extension, `.bsl`. The compiler globs `**/*.bsl` under the `based.toml` root; any
  declaration may live in any file. Recommended (not enforced) layout: one directory per domain, split
  `model.bsl` (model + its shapes) and `queries.bsl` (access layer).
- Declarations are separated by a newline **or** a comma (interchangeable).
- **`based.toml`** sets defaults: `dialect = "mariadb"`, `client = "rust"`, `[schema] id = "uuid"`,
  `foreign_keys = "convention"`.
- **No implicit fields.** Nothing is added behind your back — declare `id`, timestamps, everything.

## Models & fields

```
Order {
  id:         Id
  total:      decimal(12, 2)
  status:     Status (default pending)
  note:       text?                       # `?` = nullable (fields are NOT NULL by default)
  placed_at:  timestamp
}
```

**Scalar types:** `text int bool timestamp date time bytes json uuid Id ulid serial float decimal(p,s)`

- `Id` resolves to the project default id strategy; put `id: Id` on every model.
- `decimal(p,s)` — `1 ≤ s ≤ p ≤ 38`; bare `decimal` = `decimal(38, 9)`. Rides the wire as a JSON string.
- `ulid` / `serial` are id-generation strategies (valid as `id`; `serial` = DB-generated sequential int).

**Field modifiers** (parentheses): `(unique)`, `(default <value>)`, `(default now())`,
`(default <enum-variant>)`, `(column "legacy_name")`.

**Table naming:** `snake_case(ModelName)`, never pluralized (`OrderItem` → `order_item`). A relation FK
column is `<field>_id`. Override with `@table("…")` (table) or `(column "…")` (column).

**Generated columns** — a stored derived column, written with `=` (not `:`) over the row's own columns:

```
Product {
  price:    decimal
  discount: decimal
  net = price - discount                     # → net … GENERATED ALWAYS AS (price - discount) STORED
  label = name || " (" || sku || ")"         # concat
  tier  = case when (qty > 100) then "bulk" else "unit" end
}
```

- Reuses the shape computed-expression language (`+ - * /`, `||`, `case`); STORED, so it is a real,
  **indexable** column — project / `where` / `order` / `@index` / keyset all just work.
- Type + nullability are **inferred** from the expression. Same-row only: no relation reaches, no
  aggregates, no params, no depending on another generated column. Its value is derived — assigning
  it in a create/update is an error, and a create never requires it.
- vs a shape [computed field](#shapes): that is query-local, projection-only (not stored/filterable);
  a generated column is stored data you can filter and sort by.

## Decorators

| Decorator | Purpose | Example |
|---|---|---|
| `@scope Name` | attach a standing scope (repeatable = OR; comma = AND) | `@scope Tenant` |
| `@index …` | index (see [Indexes](#indexes)) | `@index(org, status)` |
| `@created(f)` / `@updated(f)` | stamp a field on insert / update | `@created(created_at)` |
| `@soft_delete(f)` | soft-delete via a real field | `@soft_delete(deleted_at)` |
| `@sort(term…)` | default order | `@sort(placed_at desc)` |
| `@fk` / `@no_fk` | opt a relation into / out of a real FK | `placed_by: User @fk(on_delete: cascade)` |
| `@was("old")` | rename directive (field or model) | `barcode: text? @was("upc")` |
| `@key(a, b)` | natural / composite primary key | `@key(student, course)` |
| `@table` / `@schema` / `@no_id` | table name / db-schema / keyless legacy | `@table("legacy_orders")` |

FK actions: `cascade restrict set_null no_action`.

## Enums

```
enum Status   { pending, paid = "PAID", shipped }   # string (bare or explicit); = != in
enum Priority { low = 0, medium = 1, high = 2 }       # numeric; also < > <= >=
```

A variant is used bare as a value: `where (status = paid)`, `(default pending)`.

## Relations

```
Order {
  placed_by:      User                                  # forward to-one (FK column placed_by_id)
  fulfilled_by:   User?                                 # optional to-one
}
User {
  placed_orders:  Order[] (Order.placed_by)             # to-many inverse (points at the forward edge)
  invited_users:  User[]  (User.invited_by)             # self-referential is fine
}
```

- Custom join: `placed_by: User (on: orders.user_ref = users.legacy_id)`
- A composite-key target expands to a multi-column FK automatically.
- **Many-to-many** = an explicit junction model (two forward edges + two inverses); flatten it away in a
  shape (see [Shapes](#shapes)). There is no implicit-junction sugar.

## Indexes

```
@index placed_at                 # single
@index(org, status)              # composite
@index(org, user) unique         # unique
@index location using gist       # access method (btree hash gist gin brin fulltext spatial …)
@index raw("(lower(email))")     # opaque expression index
```

## Soft delete

`@soft_delete(deleted_at)` points at a real nullable `timestamp`/`date` (or `bool`) field. It auto-adds a
read filter (live rows only) and two ops; a true delete must be spelled `hard delete`.

```
delete Order where (id = $id);        # tombstone (sets the field)
restore Order where (id = $id);       # clear it
hard delete Order where (id = $id);   # real DELETE
```

Override an op's SQL on a member line: `read|delete|restore : raw`…``.

## Shapes

A shape is a named projection: `shape Name from Model { … }`.

```
shape OrderCard from Order {
  id
  total                                     # bare = local column
  city      = address.city.name             # reach + rename
  buyer     = placed_by -> UserRef          # nest via a named shape
  items     { sku, qty }                    # inline nested object
  courses   = enrollments.course { title }  # flatten a m2m junction to the far side (distinct)
  ...OrderBase                              # splice another same-model shape's fields
  net       = total - discount              # computed (+ - * /, || concat)
  tier      = case when total > 100 then "premium" else "standard" end
}
```

**Aggregate shape** (pair with [group by](#group-by)): `count()` (→ int), `sum(f)`, `avg(f)`, `min(f)`,
`max(f)`. Raw value: `full_name = raw`concat(first,' ',last)``. Shapes never filter or sort.

## Queries

```
query name(params) -> RetType [scoped Name] ;            # or { body } or inline clauses;
```

- **Return:** `ok | Shape | Shape[] | stream Shape`. Singular infers `get`, `[]` infers `list`.
- **Bare** (params become an equality-AND filter): `query orders(user, company) -> OrderCard[];`
- **Full body:** `query recent(org) -> OrderCard[] { list Order where (org = $org) order (placed_at desc); }`

**Param binding:** `(id)` same-name eq · `(user: User)` typed · `(user -> author)` bind through an edge ·
`(since: timestamp > created_at)` explicit column + operator.

**Optional filter (`name?`):** a `?` makes a param an optional filter, with **any operator** — `query search(status?, since?: timestamp > created_at) -> OrderCard[];`. Two states: **absent** drops the predicate, a **value** applies it (`=`, `~`, `>`, `in`, `has`, …). Client type is `Option<T>` (`None` skips). Works as a signature param or a `$`-ref inside a block `where` (incl. `or`-composition); `list`-only, no `= default`, not in a raw body (E0335, E0336, E0338). Null-matching is a body concern (`where col = null`), not a param state.

**Filter operators:** `= != > < >= <=`, `~` (LIKE, pattern verbatim), `in`, `has` (array/json contains).
Compose with `and` (binds tighter), `or`, `not`, parentheses. Bare bool column: `where (active)`.
Lists: `status in (open, waiting, $other)`.

**Scope acknowledgement** is mandatory on a scoped model: `scoped Tenant` or `unscoped("reason")`.

## Sorting

Precedence: query `order (…)` > relation `@sort` > model `@sort` > none (lint).

```
order (placed_at desc, id)                 # per-term asc (default) / desc
order (assignee asc nulls last)            # explicit NULL placement (nullable keys)
```

`nulls first` / `nulls last` (default: the dialect's own — Postgres nulls-last on asc, MySQL/MariaDB/SQLite nulls-first) pins where NULLs fall, portably: native `NULLS FIRST|LAST` on Postgres/SQLite, a leading `col IS NULL` term on MySQL/MariaDB. Also valid in `@sort`. Keyset pagination follows the chosen placement.

## Pagination

```
page (20)              # keyset (default); returns { rows, cursor }; `order` is the cursor basis
page (50) offset       # offset instead
page (50) with count   # also return `total`
```

## Streaming

```
query export(org) -> stream OrderCard scoped Tenant;   # no []; NDJSON wire; page forbidden
```

## Locking & distinct

```
get Order where (id = $id) for update;          # + nowait | skip locked (tx client only)
list distinct City order (region);              # SELECT DISTINCT (every order col must be projected)
```

## Named filters

Reusable predicates:

```
filter active     = not banned and deleted_at = null;
filter in_city(c) = address.city.name = $c;
query users(name) -> UserRow[] where (active and in_city($city));
```

## Group by

Query clauses that pair with an aggregate shape:

```
list Order group by (placed_by) having (revenue > 1000) order (revenue desc);
```

## Mutations

```
mutation name(params) -> RetType [guard g] [scoped Name] { stmt; … }
```

```
create Order { placed_by = $buyer, total = $total };
update Product where (id = $id) { qty = qty + $delta };   # RHS arithmetic is atomic (+ - * /)
delete Comment where (id = $id);                          # `delete all Model` wipes the table
restore Order where (id = $id);
hard delete Order where (id = $id);                      # `hard delete all Model` too
```

**Transaction** with step bindings:

```
tx {
  create User { email = $email } as user;
  create Address { user = $user.id, city = $city };   # $name.field reads a bound step's row
}
```

## Bulk & nested writes

A shape doubles as a write-input type — read a shape out, hand the same struct back in.

```
shape ProductIn from Product { sku, name, price }

mutation import(rows: ProductIn[]) -> ok scoped Tenant {
  create Product[] from $rows;          # one chunked, atomic multi-row INSERT
}
create Product from $row;                # single-row structured create
```

Nested blocks in the input shape: FK link `category { id }`; nested write to-one `customer { name, email }`
(created first, links parent); to-many inverse `items { … }` (created after, back-linked).

## Upsert

```
create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
```

The conflict target must be a declared unique key; the branch may not move it; not allowed on a
`@soft_delete` model. In a **bulk** upsert the update branch also sees the proposed row:

```
create Inventory[] from $rows
  on conflict (org, sku) update { qty = qty + incoming.qty, price = incoming.price };
```

- A bare column (`qty`) is the **stored** row; `incoming.<col>` is the **proposed** row.
- `...incoming` (bulk only, at most once) sets **every** payload column except the conflict target from
  the proposed row — the last-write-wins overwrite. Explicit assigns compose in lexical order:

```
create Card[] from $rows on conflict (oracle_id) update { ...incoming };
create Card[] from $rows on conflict (oracle_id) update { ...incoming, hits = hits + incoming.hits };
```

## Auth & scope

Three layers, smallest to largest:

1. **`$ctx`** — the request-context bag: `where (org = $ctx.org)`.
2. **Named scope** — declare once, attach to models, acknowledge per callable:

```
scope Tenant (org: Org = $ctx.org)     # a term is `col: Type = $ctx.field`
@scope Tenant                          # on the model (repeatable = OR; comma = AND)
Order { … }
mutation ship(id) -> ok scoped Tenant { … }   # `scoped Name` (or unscoped("reason")) is mandatory
```

On a `create`, scope columns are set from `$ctx` automatically (assigning one yourself is an error).

3. **`guard`** — a host-language allow/deny check, mutations only, after the return type:

```
mutation refund(id) -> RefundResult guard caller_can_refund scoped Tenant { … }
```

## Raw

The escape hatch — `raw`, never `sql`.

```
name = raw`concat(first, ' ', last)`                    # value / predicate in a shape or query
location: raw("geometry(Point, 4326)")?                 # opaque column type
@index raw("(lower(email))")                             # opaque index
query heavy(min: int) -> UserRow[] {                     # whole-query raw body
  raw`SELECT u.name AS name FROM "user" u WHERE u.total >= ${min}`;
}
```

Inside backticks: `${param}` binds a param; `{table}` / `{id}` interpolate safely.

## Migrations

Offline snapshot diff — no live-DB introspection.

```
based migrate gen        # diff last snapshot vs *.bsl -> migrations/NNNN_slug/{up.mig, schema.snap, down.mig}
based migrate apply      # --allow-destructive, --to NNNN, --down
based migrate status
based migrate verify
```

Steps are dialect-neutral (`add column`, `alter column`, `rename table`, `add index`, `add foreign_key`,
`raw(<dialect>)`…). Renames come from `@was("old")`. A ledger table (`_based_migrations`) tracks applied
migrations gap-free with tamper + drift checks.

## Client

Each query/mutation generates a typed client method + one wire endpoint (`POST /q/<name>`, JSON body);
clients call fixed signatures — the DSL never ships. Ids are phantom-typed newtypes (`Id<Order>`),
transparent over the wire. `page` returns `{ rows, cursor }` (+ `total` with count); `-> ok` returns unit;
`-> stream` returns an async `Stream` of rows over NDJSON. An embedded in-process client is available via
`client::embedded(&engine)`.
