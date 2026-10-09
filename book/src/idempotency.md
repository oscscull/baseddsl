# Retrying mutations

Database mode is the default for `based serve`. A mutation's key, SQL effects,
and response commit together. Retrying the same callable, key, arguments, and
trusted context replays the retained result across instances and process restarts.
Changed arguments or context return `422 idempotency_key_reuse`.

Supply `Idempotency-Key` for requests you may retry. Keys are scoped to a
callable and database/shard, not a tenant: use globally unique keys and route
retries to the same database. This protects database effects, not external side effects.
Unkeyed calls execute each time.

Each shard needs `_based_idempotency`. Provision it through reviewed DDL from
`based_codegen::sql::idempotency_table_ddl`, or explicitly run
`based serve --init-idempotency-table`. Ordinary startup checks the table and
fails if it is missing. Runtime access requires SELECT, INSERT, and UPDATE;
initialization additionally needs CREATE TABLE.

Records remain until deleted. Keep them for your full retry horizon and budget
storage for retained response bodies. Deleting a record permits execution again;
cleanup must avoid active transactions.

| Store | Replay boundary |
|---|---|
| `database` | Atomic database effect and response, shared across restarts |
| `memory` | Process-local, 24-hour TTL, concurrent duplicate returns 409; commit-to-record gap can duplicate effects |
| `none` | Every request executes, including keyed requests |

Select alternatives explicitly with `--idempotency-store`. Store selection
does not authenticate callers; [guards](guards.md) still run before keyed replay.
