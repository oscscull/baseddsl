# axum-helpdesk

A multi-tenant **support desk** — requester portal, agent desk, ops/finance — built the way
a backend team would actually build it: an ordinary **axum** service that **embeds the
engine**. The app owns its own sqlx `PgPool`; the engine runs over it
(`PgRouter::from_pool`), so the app's queries and the engine's share one set of
connections; every handler is a single call on the **generated typed async client**.

This is the full-surface example. The [quickstarts](../sqlite-quickstart) are the minimal
first run; this desk is the guided tour of the language — scoped multi-tenancy, enums,
decimals, ordered nested shapes, a host-code `guard`, keyed (idempotent) writes, a
streaming NDJSON export, raw-SQL leaves, migrations with a data-preserving rename.

## The pieces

| path | what it is |
|---|---|
| `schema/` | the whole desk in `.bsl`, by domain — `ticket/model.bsl` + `ticket/queries.bsl`, etc. |
| `migrations/` | checked-in artifacts of `based migrate gen` — `0002` renames a column via `@was`, preserving data |
| `src/client.rs` | **verbatim** output of `based gen client -o src/client.rs --embedded`; regenerate after a schema change, never edit |
| `src/app.rs` | the wiring: the app's `PgPool` → `PgRouter::from_pool` → `Engine`, plus the close-policy guard |
| `src/auth.rs` | bearer middleware: the token resolves to a session **through the client itself** |
| `src/routes.rs` | the HTTP surface — one typed call per handler |
| `src/bin/seed.rs` | demo data through the client's own mutations; prints the bearer tokens |
| `src/bin/smoke.rs` | the CI gate: boots the service and drives every route over real HTTP |

## Run it

The steps shell out to the `based` CLI, so install it onto your `PATH` once, and start a
throwaway Postgres (port 15432) — both from the repo root:

```sh
make dev-db-up
cargo install --path crates/based-cli
```

Then, from this directory (`examples/axum-helpdesk`):

```sh
# 1. DATABASE_URL is set in .env (it matches `make dev-db-up`). Edit it for your own server.
set -a; source .env; set +a

# 2. Create the tables from the checked-in migrations.
based migrate apply --database-url "$DATABASE_URL"

# 3. Demo data — two tenants, agents and requesters, tickets in every state.
cargo run --bin seed

# 4. The desk, on http://127.0.0.1:8000.
cargo run
```

The seed ends by printing the demo bearer tokens the service's auth middleware resolves:

```
demo bearer tokens:
  acme    agent      Mara   tok-acme-mara
  acme    agent      Noah   tok-acme-noah
  acme    requester  Ada    tok-acme-ada
  ...
```

Try it (from another terminal):

```sh
# Ada — a requester — sees exactly her own tickets, nobody else's:
curl -s http://127.0.0.1:8000/my/tickets -H 'Authorization: Bearer tok-acme-ada'

# Mara's queue: assigned to her, still open, urgent by rank or sitting too long:
curl -s http://127.0.0.1:8000/queue -H 'Authorization: Bearer tok-acme-mara'

# Open a ticket with a retry-safe key. Run it twice: the retry replays the first
# response — same id, one row ever written.
curl -s -X POST http://127.0.0.1:8000/tickets \
  -H 'Authorization: Bearer tok-acme-ada' \
  -H 'Idempotency-Key: demo-1' -H 'Content-Type: application/json' \
  -d '{"subject":"Printer on fire","body":"Actual flames."}'

# Closing an unresolved ticket is denied by the app's own close policy —
# 403 {"error":{"code":"guard_denied","message":"only a resolved ticket can be closed"}}.
# (Take any id from Mara's queue above.)
curl -s -X POST http://127.0.0.1:8000/tickets/<id>/close -H 'Authorization: Bearer tok-acme-mara'

# The compliance export streams NDJSON: one {"row":…} per line, then a terminal
# {"done":{"rows":N}} checksum line.
curl -sN http://127.0.0.1:8000/export/tickets.ndjson -H 'Authorization: Bearer tok-acme-mara'
```

To start over: `cargo run --bin smoke -- reset` drops and recreates the schema; then repeat
from step 2. `cargo run --bin smoke` (against a freshly seeded database) is the full gate
CI runs — every route, over real HTTP.

## The tour — read the schema first

The service is thin on purpose; the desk's behavior lives in `schema/`. The shortest
rewarding path through it:

**1. Tenancy is declared once, then referenced by name** (`schema/org/model.bsl`,
`schema/ticket/model.bsl`). A `scope` is a named row-visibility contract; models opt in
with `@scope`, and every callable that touches a scoped model must acknowledge it — the
compiler rejects one that doesn't:

```
scope Tenant (org: Org = $ctx.org)
```

`Ticket` stacks two scopes — **stacked `@scope` lines are OR** (agents see the whole org,
requesters see their own tickets); the agent's private `DraftNote` declares
`@scope Tenant, Author` — **one line, two names, is AND**. Cross-tenant access is
inexpressible without a greppable `unscoped("reason")`; this app has exactly four, all in
auth and ops, each carrying its reason.

**2. The ticket model reads like the domain** (`schema/ticket/model.bsl`): a string enum
whose `waiting` variant maps to a legacy stored value, an ordered int `Priority`,
soft-delete as archive/restore, a self-referential `duplicate_of`/`duplicates` pair, and
nested projections whose arrays come back **in declared sort order** — comments
chronological (the relation's `@sort`), time entries by when the work happened (the child
model's `@sort`).

**3. Queries are signatures; the common case has no body**
(`schema/ticket/queries.bsl`):

```
query ticket(id) -> TicketDetail scoped Tenant;
query tickets_for(agent -> assignee, since: timestamp > created_at) -> TicketRow[] scoped Tenant;

query queue() -> TicketRow[] scoped Tenant { list Ticket where (assignee = $ctx.user and open_states and (priority >= high or overdue)); }
```

`priority >= high` is an ordered comparison on an int enum. `open_states` is a named,
reusable filter; `overdue` is a **raw-SQL leaf** (interval math the DSL doesn't model) that
composes as one boolean term — scope and soft-delete still wrap the query around it. In
`search_tickets`, `~` is verbatim SQL `LIKE`, so the route handler wraps the user's
substring in `%…%` (`src/routes.rs`).

**4. One mutation, one transaction** — `open_ticket` creates the ticket and its first
comment atomically; `^` reads the row the preceding `create` produced. On the wire this is
`POST /tickets`, and a caller supplying `Idempotency-Key` gets the generated
`open_ticket_with_key` twin: a retried POST replays the first response. The one
*destructive* mutation — `purge_comment`, legal/PII removal — is a `hard delete`
returning the bare `-> ok` acknowledgement: no row survives to read back, so
`DELETE /admin/comments/{id}` answers `200` with an empty body, and a missing or
cross-tenant id is the engine's own `404 not_found`.

**5. Real authorization decisions stay in your code** — `close_ticket` declares
`guard caller_can_close`, and the engine refuses to build until the app registers an
implementation (`src/app.rs`). The engine owns *that* the check runs — before the write, on
every door; the app owns *what it decides* (here: only a resolved, visible ticket closes;
a check that cannot decide denies). The decision is host code, but the *state it reads*
comes back through the schema's own `ticket` query over `req.engine()` — so the workspace
scope and soft-delete filter are the ones the schema declares, not hand-written SQL.

**6. The export is a stream** — `export_tickets` returns `-> stream TicketExport`: the
client method yields a typed `RowStream` (rows arrive as the database produces them), and
the route re-serves it as NDJSON. A client that disconnects cancels the database pass.
`age_days` in the export shape is a raw-SQL value leaf.

**7. Auth is dogfooding** — `src/auth.rs` trades `Authorization: Bearer <token>` for a
session by calling `session_by_token` — a query in the same schema — so everything handlers
later pass as `$ctx` is **derived server-side**, never read from a request body.

## Where each idea is specified

| in this example | spec |
|---|---|
| models, enums, decimal/float, relations | `spec/syntax/models.md`, `enums.md`, `relations.md` |
| `scope` / `@scope` / `scoped` / `unscoped`, `$ctx`, `guard` | `spec/syntax/auth.md` |
| queries, bare + per-param + full-body forms, named filters | `spec/syntax/queries.md` |
| mutations, `tx` + `^`, idempotency keys | `spec/syntax/mutations.md` |
| shapes, nested projections, `-> UserRef` | `spec/syntax/shapes.md` |
| sort cascade (model / relation / query) | `spec/syntax/sorting.md` |
| keyset + offset pagination | `spec/syntax/pagination.md` |
| `-> stream`, the NDJSON wire | `spec/syntax/streaming.md` |
| soft-delete, `restore`, `hard delete` + `-> ok` | `spec/syntax/soft-delete.md`, `mutations.md` |
| `@index`, `unindexed(...)`, the index lints | `spec/syntax/indexing.md` |
| raw-SQL leaves | `spec/syntax/raw.md` |
| migrations, `@was` renames | `spec/syntax/migrations.md` |
| the generated client, typed ids, errors | `spec/syntax/calling.md` |

> Standalone crate, **outside** the cargo workspace (the root `Cargo.toml` `exclude`s
> `examples/`). It depends on the in-repo engine crates by path, so it always tracks the
> current engine, but `cargo test --workspace` never builds it.
