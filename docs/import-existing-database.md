# Import models from an existing database

`based import` performs one metadata read and writes checked BSL models. It supports
MariaDB 11.4, PostgreSQL 16 and an existing SQLite file. Your current migration tool
keeps ownership of the database. Import does not run SQL migrations, generate a
baseline ledger, create a database, or generate a client implicitly.

Use the matching CLI/runtime commit from the [installation guide](installation.md).
The default CLI includes all three readers; a source build with
`--no-default-features` includes only SQLite. Import is an explicit adoption step,
not a sync or merge engine.

## Configure a project

Create `models/` and a `based.toml` in your consumer project:

```toml
dialect = "sqlite"
root = "models"
[generate]
client = "src/based_client.rs"
client_mode = "embedded"
```

Use `mariadb` or `postgres` for a server database. This workflow starts with your
existing database and a configured source directory. The blank-project initializer
is a separate workflow that includes its own starter schema and migration.

Set `BASED_DATABASE_URL` in the environment or the project's untracked `.env`.
An explicit `--database-url` takes precedence, but environment configuration avoids
putting credentials in shell history/process arguments. Exactly one connection is
required; a shard list is rejected. Relative SQLite paths resolve from the project
root, including when invoked in a subdirectory.

```sh
export BASED_DATABASE_URL=/absolute/path/original.db
based import --table main.legacy_account --table main.legacy_entry --json
```

Every table argument is an explicit `namespace.table`; SQLite's namespace is
`main`. For PostgreSQL use the actual schema; for MariaDB use the actual database:

```sh
based import --table public.legacy_account --table public.legacy_entry --json
based import --table app_database.legacy_account --table app_database.legacy_entry --json
```

Run the command matching the manifest dialect and connection. Table selection is
closed: selecting a table does not automatically select its foreign-key targets.
Include every referenced target. No wildcard or inferred relation is supported.

Server connections preserve SQLx URL TLS options, including CA and hostname
verification; see [database TLS](database-tls.md). Use a metadata account with no
application-row read or mutation privileges: PostgreSQL schema `USAGE` plus catalog
visibility, or MariaDB whole-table/database `REFERENCES` visibility. A SQLite reader
opens the existing file read-only, denies application-row reads and never changes
journal mode or checkpoints its WAL. Keep MariaDB DDL quiescent during import:
its reader compares catalog passes and rejects drift, without promising an MVCC
DDL snapshot.

## Review the report and the models

Successful JSON has `status: "imported"`, the physical `catalog`, catalog
`diagnostics`, compiler findings, `written` paths and external `migration_ownership`.
Without `--json`, findings are rendered for people. Native definitions and defaults
remain in the report; review that source material before sharing it.

An unsupported type, generated expression, CHECK, index facet or relation blocks
publication instead of inventing a mapping. Partial selection, an existing-model
conflict, or any destination collision also blocks the complete staged set. See
[the catalog contract](import-catalog-contract.md) and
[the emitter's supported matrix](catalog-bsl-emitter.md) for exact boundaries.
Warnings explain native storage/DDL details that a BSL model cannot reproduce.
Treat them as review work before considering future DDL.

Names and physical aliases are deterministic. Declared keys and relations are
preserved; import does not infer `@owner`, guards, soft deletion or business policy.
After publication, these models are hand-owned source. Repeat import refuses every
existing destination, even if its bytes match, and cannot replace your edits.

`--output models/adopted` chooses a directory inside the configured schema root.
Symlink destinations and migration/configuration-history directories are rejected.
There is no overwrite flag. Import stages the complete model set, checks it together
with existing source, and publishes files exclusively. A filesystem failure during
publication can leave only a subset: `status: "partial"` lists the exact files
written. Review those files before retrying; multi-file publication is not a
filesystem transaction.

## Add a hand-written read and generate the client

The independently authored acceptance fixture has:

- `legacy_account(account_code TEXT PRIMARY KEY, label TEXT)`;
- `legacy_entry(entry_key BIGINT PRIMARY KEY, parent_code TEXT, heading TEXT)`;
- a required foreign key from `parent_code` to `account_code`, with delete RESTRICT
  and update CASCADE, plus an existing lookup index.

The keys are application-supplied, and required columns have explicit `NOT NULL`.
Server fixtures use their corresponding native declarations. The
[hand SQL](../crates/based-cli/tests/import_support/fixture.sql) creates and seeds
this database independently of Based. Import emits `LegacyAccount` and
`LegacyEntry`, retaining physical aliases. Write `models/reads.bsl`:

```bsl
shape LegacyRow from LegacyEntry { heading parent_code { label } }
query imported_entries() -> LegacyRow[] {
  list LegacyEntry order (entry_key asc);
}
```

```sh
based check
based gen client --embedded
```

Client generation does not connect to the database. Add `based-runtime` pinned to
the same full Git commit as your CLI, `serde` with derive, `serde_json`, Tokio with
macros/runtime support, and SQLx 0.9 with the matching driver. Use runtime features
`sqlite,id-gen`, or `postgres,id-gen,tls-rustls` /
`mariadb,id-gen,tls-rustls` for servers. A server SQLx dependency also needs
`tls-rustls-ring-webpki`; see the
[tested consumer manifest renderer](../ci/import_workflow/rust.py).

Open a caller-owned read-only/SELECT-only pool against the original database.
For SQLite, `src/database.rs` is:

```rust
use sqlx::ConnectOptions;

pub async fn open(path: &str) -> Result<based_runtime::SqliteBackend, Box<dyn std::error::Error>> {
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(path)
        .read_only(true).create_if_missing(false).pragma("query_only", "ON")
        .disable_statement_logging();
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect_with(options).await?;
    Ok(based_runtime::SqliteBackend::from_pool(pool))
}
```

For servers, parse the SELECT-only connection URL into `PgConnectOptions` or
`MySqlConnectOptions`, preserving its TLS settings, and pass the resulting pool to
`PgRouter::from_pool` or `driver::ShardRouter::from_pool`. The
[consumer renderer](../ci/import_workflow/rust.py) contains both small adapters.
Metadata and application SELECT permissions are separate responsibilities.

Use the generated client in `src/main.rs`:

```rust
mod database;
#[allow(dead_code)]
mod client { include!("based_client.rs"); }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let compiled = based_runtime::Compiled::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        .map_err(|error| format!("compile imported models: {error:?}"))?;
    let database = database::open(&std::env::var("DATABASE_URL")?).await?;
    let engine = based_runtime::Engine::new(compiled, database, based_runtime::id::UuidGen);
    let rows: Vec<client::LegacyRow> = client::embedded(&engine)
        .imported_entries(client::ImportedEntriesInput {}, ()).await?;
    println!("imported read: {}", serde_json::to_string(&rows)?);
    Ok(())
}
```

Set `DATABASE_URL` to the original SQLite path or the server's SELECT-only URL and
run `cargo run`. This fixture returns the retained heading and nested account
label. No `migrate apply`, bootstrap, baseline, `CREATE TABLE` or build script is
part of the consumer path. Existing migrations continue to live with their current
owner. Future schema changes require a separate, explicitly reviewed adoption
strategy; import does not establish one.

## Verification

The existing onboarding job runs `ci/check-import-consumer.py` in a fresh consumer,
using the exact Git commit for its runtime dependency. The native source-consumer
job repeats it on the Rust 1.94 floor. Existing live MariaDB/PostgreSQL jobs run the
same typed consumer against independent hand-SQL fixtures, with metadata-only and
SELECT-only accounts. Their administrator compares schema definitions, constraints,
indexes, tables and seeded rows before/after; SQLite compares the whole original
file. The existing TLS gate also runs these server proofs with verified TLS URLs.

CLI integration tests cover unsupported definitions, partial selection, combined
source failures, collision refusal, hand-edit preservation, deterministic output,
subdirectory discovery, missing-file redaction and symlink rejection. All run within
existing CI tiers; import adds no separate service matrix.
