//! End-to-end integration against a **real** Postgres server, over Docker.
//!
//! The Postgres twin of `mariadb_integration.rs`: it loads the *actual* commerce schema (the
//! same discover → parse → check front end the CLI uses), lowers it for **`Dialect::Postgres`**
//! (so the DML binds `$n` — *not* the manifest's `mariadb`), creates its tables from the
//! *generated* Postgres DDL (`sql::ddl(_, Dialect::Postgres)`), and drives real requests
//! through `serve::dispatch` against a live `PgRouter` (the concrete Postgres `Backend`).
//! What runs is the *verbatim* codegen-lowered Postgres SQL — bound positionally
//! (`$1, $2, …`) by the runtime — so a passing test proves the whole engine (the `PostgresDb`
//! `Db`/`Backend`/`ping` seams, the `SqlValue`↔Postgres value mapping incl. uuid/timestamptz/
//! jsonb round-trip) works against a genuine server, not just compile-verified.
//!
//! Like the MariaDB suite this needs infra: an ephemeral Postgres container. The harness
//! ([`docker_postgres`]) starts one on a random port and tears it down after; when the Docker
//! daemon is unreachable it returns `None` and **each test skips cleanly** (logs + early-
//! returns), so `cargo test --workspace --all-features` stays green with no daemon. This suite
//! is the `PostgresDb` driver's real gate: it exercises the SQL a live Postgres actually runs.

#![cfg(feature = "docker-tests")]

#[path = "support/docker_postgres.rs"]
mod docker_postgres;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::id::UuidGen;
use based_runtime::idempotency::{DbStore, MemStore, NoStore};
use based_runtime::run::{Backend, Db, DbError, DbErrorKind, DbRead};
use based_runtime::shard::PoolConfig;
use based_runtime::{dispatch, fetch_all, Compiled, Guards, PgRouter};
use based_sema::check;

use docker_postgres::PostgresContainer;

// Valid v4-shaped UUIDs for the seed rows — Postgres's native `uuid` column (which the
// generated DDL emits) rejects a non-UUID string, so the fixtures use real UUID literals. The
// trailing digits keep them human-readable across the assertions.
const ORG_1: &str = "00000000-0000-4000-8000-0000000000a1";
const USER_1: &str = "00000000-0000-4000-8000-0000000000b1";
const ORDER_1: &str = "00000000-0000-4000-8000-0000000000c1";

/// Bring up a live Postgres, load commerce **lowered for Postgres**, create the generated
/// Postgres DDL, seed a couple of rows, and return the router (the live `Backend`) alongside
/// the loaded schema. Returns `None` when Docker is unavailable — the caller skips. The
/// container's lifetime is tied to the returned guard, so the caller must hold it.
async fn live() -> Option<(Compiled, PgRouter, PostgresContainer)> {
    let container = PostgresContainer::start().await?;

    // Load the commerce front end, then lower it for **Postgres** explicitly. The commerce
    // manifest's dialect is `mariadb`, so `Compiled::load` would lower `?`-bound MariaDB SQL;
    // here the dialect genuinely matters (Postgres binds `$n`, quotes with `"`, and has real
    // `uuid`/`jsonb`), so we re-lower via `from_checked(_, Dialect::Postgres)`.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/examples/commerce")
        .canonicalize()
        .expect("commerce example dir");
    let project = based_manifest::discover(&root).expect("discover commerce");
    let mut decls = Vec::new();
    for (i, f) in project.files.iter().enumerate() {
        let src = std::fs::read_to_string(&f.path).expect("read bsl");
        let sf = parse_file(&src, FileId(i as u32)).expect("parse bsl");
        decls.extend(sf.decls);
    }
    let (schema, diags) = check(&decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260"),
        "commerce must check clean: {diags:?}"
    );
    let compiled = Compiled::from_checked(schema, decls, Dialect::Postgres);
    assert_eq!(compiled.dialect, Dialect::Postgres);

    let router = PgRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));

    // Create every commerce table from the *generated* Postgres DDL (not a hand copy), then
    // seed fixtures — so this suite exercises the whole `based gen sql` artifact (DDL + DML).
    let ddl = sql::ddl(&compiled.schema, Dialect::Postgres);
    container.exec_batch(RESET_SQL).await;
    container.exec_batch(&ddl).await;
    container
        .exec_batch(&format!(
            // `total` is NUMERIC(12,2) (returned as its exact string); ids/uuids ride as text literals Postgres coerces into `uuid`;
            // `deleted_at` defaults NULL (live rows).
            "INSERT INTO \"org\" (\"id\", \"name\", \"slug\") VALUES ('{ORG_1}', 'Acme', 'acme');\n\
             INSERT INTO \"user\" (\"id\", \"email\", \"name\") VALUES ('{USER_1}', 'a@x.com', 'Ada');\n\
             INSERT INTO \"order\" (\"id\", \"org_id\", \"placed_by_id\", \"status\", \"total\")\n\
                 VALUES ('{ORDER_1}', '{ORG_1}', '{USER_1}', 'paid', 500.00);"
        ))
        .await;

    Some((compiled, router, container))
}

/// Compile an in-line schema **lowered for Postgres** (skip disk). The pagination +
/// soft-delete/restore suites need small, self-contained schemas rather than the whole commerce
/// topology, so the tested behaviour is the only variable.
fn compile(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260"),
        "schema must check clean: {diags:?}"
    );
    Compiled::from_checked(schema, sf.decls, Dialect::Postgres)
}

/// Bring up a live Postgres, compile an in-line schema for Postgres, and create its tables from
/// the generated Postgres DDL — returning the router + schema + container for a test to seed and
/// drive. Returns `None` when Docker is unavailable (the caller skips). The `id: text` columns
/// these schemas declare map to `TEXT`, so the fixtures use plain string ids.
async fn live_schema(src: &str) -> Option<(Compiled, PgRouter, PostgresContainer)> {
    let container = PostgresContainer::start().await?;
    let compiled = compile(src);
    let router = PgRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));
    let ddl = sql::ddl(&compiled.schema, Dialect::Postgres);
    container.exec_batch(RESET_SQL).await;
    container.exec_batch(&ddl).await;
    Some((compiled, router, container))
}

/// Drop and recreate the `public` schema before creating tables, so a suite run against a
/// *persistent* external server (`TEST_POSTGRES_URL`) starts clean and is re-runnable.
/// `CASCADE` clears tables + the `_based_migrations` ledger in one step; a no-op-equivalent
/// against a fresh self-spun container.
const RESET_SQL: &str = "DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;";

/// Run one request through the real dispatch core against the live router — the exact path
/// `based serve` uses, minus the socket (dispatch checks its own connection out of the
/// `Backend`).
async fn call(
    compiled: &Compiled,
    router: &PgRouter,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
) -> based_runtime::WireResponse {
    let ids = UuidGen;
    dispatch(
        compiled,
        router,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        method,
        path,
        args,
        ctx,
        None,
    )
    .await
}

fn sort_items(mut o: serde_json::Value) -> serde_json::Value {
    if let Some(items) = o.get_mut("items").and_then(|v| v.as_array_mut()) {
        items.sort_by_key(|r| r["sku"].as_str().unwrap_or_default().to_string());
    }
    o
}

#[path = "postgres_integration/backend.rs"]
mod backend;
#[path = "postgres_integration/hardening.rs"]
mod hardening;
#[path = "postgres_integration/locking_and_adopt.rs"]
mod locking_and_adopt;
#[path = "postgres_integration/mutations.rs"]
mod mutations;
#[path = "postgres_integration/nesting_and_serial.rs"]
mod nesting_and_serial;
#[path = "postgres_integration/optional_filters.rs"]
mod optional_filters;
#[path = "postgres_integration/pagination_and_types.rs"]
mod pagination_and_types;
#[path = "postgres_integration/queries.rs"]
mod queries;
#[path = "postgres_integration/schemas_and_writes.rs"]
mod schemas_and_writes;
#[path = "postgres_integration/scopes_and_idempotency.rs"]
mod scopes_and_idempotency;
#[path = "postgres_integration/streaming.rs"]
mod streaming;
