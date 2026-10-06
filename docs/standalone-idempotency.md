# Keyed mutations in `based serve`

`based serve` defaults to `--idempotency-store database`. It uses the existing
transactional `DbStore`: the key, SQL effects and response commit together in the
same database. Matching retries replay a retained result across instances and
process restarts; concurrent duplicates block until the first transaction finishes.
An ambiguous commit is resolved by retrying the same key, args and trusted context.
This covers database effects, not arbitrary external side effects or guaranteed success.

Store selection does not affect queries or unkeyed mutation semantics. Unkeyed
calls execute every time. Keys are `(callable, key)` within a database/shard, not
per tenant: use globally unique keys. Changed args or trusted context with a retained
key return `422 idempotency_key_reuse`. Every retry must route to the same database;
resharding must preserve the key records alongside their protected data.

## Explicit table setup

Each physical database/shard needs `_based_idempotency`, with the schema produced by
`based_codegen::sql::idempotency_table_ddl` for its dialect. The default checks that
its columns exist before listening and fails with setup guidance if they do not.
It does not create or alter a table silently.

For initial setup with a role allowed to create tables:

```sh
based serve --init-idempotency-table
```

This explicitly runs `CREATE TABLE IF NOT EXISTS` on **every configured shard**,
then serves normally. It creates only the store table, not application tables;
application migrations remain explicit. Provision one database at a time through a
reviewed migration if that is your deployment policy, then run ordinary `based serve`
with the restricted runtime role. The shared DDL helper is available to Rust migration
owners; a one-time privileged initializer can also use the flag above. Existing tables
are not automatically upgraded or repaired: preserve the generated primary key and
column contract. Partial setup across shards is possible on failure; re-running explicit
initialization safely completes missing tables and does not clear keys.

Keyed runtime access requires SELECT, INSERT and UPDATE. CREATE TABLE is required only
for explicit initialization; operator-managed cleanup additionally needs DELETE.
Keep the store table in the same database as its mutations. All instances sharing that
database see its records. SQLite uses one file; multiple database URLs are rejected.
Server-driver shards are checked/provisioned individually. The process must reach every
configured shard at startup for database mode. A missing table or connection failure
prevents startup; diagnostics do not display database URLs/credentials.

## Retention and capacity

The CLI database mode retains keys **until explicitly deleted**. It runs no automatic
GC and has no retention-duration flag. Plan retention and disk capacity before deployment:
rough retained key count is new keyed mutations per second × retention duration; full
responses are stored, so response sizes and index/log overhead matter too.

Deleting a key permits the same request to execute again. Cleanup must preserve the full
retry horizon and avoid active transactions. Use reviewed, bounded maintenance and
monitor oldest records, cleanup failures, table/index size and pool pressure. The current
library `DbStore::with_gc` is a basic best-effort single-shard helper; it is not selected
by the CLI and is not a verified high-volume cleanup strategy. Retention and cleanup
hardening are separate work, not implied by exposing the existing store here.

## Local and no-store modes

| CLI selection | guarantee | retention |
|---|---|---|
| `database` (default) | atomic database effect and response replay across instances/restarts | until explicitly deleted |
| `memory` | process-local replay; concurrent duplicate returns 409; commit-to-record gap permits duplicates | 24-hour TTL; lost on restart; TTL must exceed operation duration |
| `none` | no deduplication, even when a key is supplied | no records |

```sh
based serve --idempotency-store memory   # local development without store-table setup
based serve --idempotency-store none     # explicitly opt out of replay
```

`--init-idempotency-table` is valid only with database mode. Deployments can set
`BASED_IDEMPOTENCY_STORE=database|memory|none` and
`BASED_INIT_IDEMPOTENCY_TABLE=true` instead of flags; explicit flags override store
selection from the environment. These options read process environment, not local `.env`
files. Every mode reports its active store and boundary on startup.

The listener trusts upstream `X-Based-Context`; a trusted auth edge must strip caller
copies, inject server-derived context, and restrict direct listener access. Store selection
is not authentication. Declared guards retain their existing startup enforcement.
Use [database TLS configuration](database-tls.md) for certificate-verified server connections.

Verification: `cargo test -p based-cli --no-default-features --test serve_idempotency`
boots separate SQLite CLI processes and exercises HTTP retries, concurrency, restart,
changed args/context, missing-table startup, explicit initialization, and local/no-store
boundaries. `make ci-standalone-store` uses disposable admin database URLs to create/drop two
uniquely named databases per server driver and proves provisioning on every shard,
concurrent HTTP retries, and restart replay. Both live CI jobs run their driver
scenario. The shared runtime live tests continue to cover transactional semantics. `make check` remains the required execution gate.
