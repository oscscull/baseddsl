//! End-to-end integration against a **real** MariaDB server, over Docker.
//!
//! This is the MariaDB twin of `sqlite_integration.rs`: it loads the *actual* commerce
//! schema (`Compiled::load` — the same discover → parse → check front end + codegen lowering
//! the CLI uses), creates its tables from the *generated* MariaDB DDL (`based gen sql` with
//! `Dialect::MariaDb`), and drives real requests through `serve::dispatch` against a live
//! `ShardRouter` (the concrete MariaDB `Backend`). What runs is the *verbatim*
//! codegen-lowered SQL — bound positionally (`?`) by the runtime — so a passing test proves
//! the whole engine (the `MariaDb` `Db`/`Backend`/`ping` seams) works against a
//! genuine server, not just compile-verified.
//!
//! Unlike SQLite this needs infra: an ephemeral MariaDB container. The harness
//! ([`support::docker_mariadb`]) starts one on a random port and tears it down after; when
//! the Docker daemon is unreachable it returns `None` and **each test skips cleanly**
//! (logs, early-returns), so `cargo test --workspace --all-features` stays green with no daemon.
//! The suite is the driver's real gate: it exercises the SQL a live MariaDB actually runs.

#![cfg(feature = "docker-tests")]

#[path = "support/docker_mariadb.rs"]
mod docker_mariadb;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::driver::{PoolConfig, ShardRouter};
use based_runtime::id::UuidGen;
use based_runtime::idempotency::{DbStore, MemStore, NoStore};
use based_runtime::run::{Backend, Db, DbError, DbErrorKind, DbRead};
use based_runtime::{dispatch, fetch_all, Compiled, Guards};
use based_sema::check;

use docker_mariadb::MariaDbContainer;

// Valid v4-shaped UUIDs for the seed rows — the generated `id`/FK columns are `CHAR(36)`
// holding the app-minted v4 string, so the fixtures use real 36-char UUID literals. The
// trailing digits keep them human-readable across the assertions.
const ORG_1: &str = "00000000-0000-4000-8000-0000000000a1";
const USER_1: &str = "00000000-0000-4000-8000-0000000000b1";
const ORDER_1: &str = "00000000-0000-4000-8000-0000000000c1";

/// Bring up a live MariaDB, load commerce, create the generated MariaDB DDL, seed a couple
/// of rows, and return the router (the live `Backend`) alongside the loaded schema. Returns
/// `None` when Docker is unavailable — the caller skips. The container's lifetime is tied to
/// the returned guard, so the caller must hold it for the test's duration.
async fn live() -> Option<(Compiled, ShardRouter, MariaDbContainer)> {
    let container = MariaDbContainer::start().await?;

    // The commerce manifest's dialect is `mariadb`, so `Compiled::load` lowers the DML for
    // MariaDB (`?` binds) — exactly the SQL this driver must run.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/examples/commerce")
        .canonicalize()
        .expect("commerce example dir");
    let compiled = Compiled::load(&root).unwrap_or_else(|e| panic!("commerce did not load: {e:?}"));
    assert_eq!(
        compiled.dialect,
        Dialect::MariaDb,
        "commerce is a MariaDB project"
    );

    let router = ShardRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live MariaDB: {e:?}"));

    // Create every commerce table from the *generated* MariaDB DDL (not a hand copy), then
    // seed fixtures — so this suite exercises the whole `based gen sql` artifact (DDL + DML).
    reset_tables(&container, &compiled).await;
    let ddl = sql::ddl(&compiled.schema, Dialect::MariaDb);
    container.exec_batch(&ddl).await;
    container
        .exec_batch(
            // `total` is DECIMAL(12,2) (returned as its exact string); ids/uuids ride as text
            // (36-char v4 literals in the `CHAR(36)` id columns). `deleted_at` defaults NULL (live rows).
            &format!(
                "INSERT INTO `org` (`id`, `name`, `slug`) VALUES ('{ORG_1}', 'Acme', 'acme');\n\
                 INSERT INTO `user` (`id`, `email`, `name`) VALUES ('{USER_1}', 'a@x.com', 'Ada');\n\
                 INSERT INTO `order` (`id`, `org_id`, `placed_by_id`, `status`, `total`)\n\
                     VALUES ('{ORDER_1}', '{ORG_1}', '{USER_1}', 'paid', 500.00);"
            ),
        )
        .await;

    Some((compiled, router, container))
}

/// Compile an in-line schema for a dialect (skip disk), mirroring `Compiled::from_checked`.
/// The pagination + soft-delete/restore suites need small, self-contained schemas rather than
/// the whole commerce topology, so the tested behaviour is the only variable.
fn compile(src: &str, dialect: Dialect) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260"),
        "schema must check clean: {diags:?}"
    );
    Compiled::from_checked(schema, sf.decls, dialect)
}

/// Bring up a live MariaDB, compile an in-line schema **lowered for MariaDB**, and create its
/// tables from the generated MariaDB DDL — returning the router + schema for a test to seed and
/// drive. Returns `None` when Docker is unavailable (the caller skips). The `id: text` columns
/// these schemas declare map to `VARCHAR(255)`, so the fixtures use plain string ids.
async fn live_schema(src: &str) -> Option<(Compiled, ShardRouter, MariaDbContainer)> {
    let container = MariaDbContainer::start().await?;
    let compiled = compile(src, Dialect::MariaDb);
    let router = ShardRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live MariaDB: {e:?}"));
    reset_tables(&container, &compiled).await;
    let ddl = sql::ddl(&compiled.schema, Dialect::MariaDb);
    container.exec_batch(&ddl).await;
    Some((compiled, router, container))
}

/// Drop this schema's tables (+ the migrations ledger) before recreating them, so a suite run
/// against a *persistent* external server (`TEST_MARIADB_URL`) starts clean and is
/// re-runnable. A no-op against a fresh self-spun container (nothing exists yet). FK checks are
/// disabled for the drop so relation order doesn't matter; the whole batch runs on one
/// connection (session-scoped `FOREIGN_KEY_CHECKS`), which `exec_batch` guarantees.
async fn reset_tables(container: &MariaDbContainer, compiled: &Compiled) {
    let mut script = String::from("SET FOREIGN_KEY_CHECKS = 0;\n");
    for m in &compiled.schema.models {
        script.push_str(&format!("DROP TABLE IF EXISTS `{}`;\n", m.table));
    }
    script.push_str("DROP TABLE IF EXISTS `_based_migrations`;\n");
    script.push_str("SET FOREIGN_KEY_CHECKS = 1;\n");
    container.exec_batch(&script).await;
}

/// Run one request through the real dispatch core against the live router — the exact path
/// `based serve` uses, minus the socket (dispatch checks its own connection out of the
/// `Backend`).
async fn call(
    compiled: &Compiled,
    router: &ShardRouter,
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

#[path = "mariadb_integration/backend_and_pagination.rs"]
mod backend_and_pagination;
#[path = "mariadb_integration/bulk_upserts.rs"]
mod bulk_upserts;
#[path = "mariadb_integration/hardening.rs"]
mod hardening;
#[path = "mariadb_integration/idempotency.rs"]
mod idempotency;
#[path = "mariadb_integration/mutations_and_scopes.rs"]
mod mutations_and_scopes;
#[path = "mariadb_integration/nested_writes_and_filters.rs"]
mod nested_writes_and_filters;
#[path = "mariadb_integration/queries.rs"]
mod queries;
#[path = "mariadb_integration/streaming.rs"]
mod streaming;
#[path = "mariadb_integration/types_and_tx.rs"]
mod types_and_tx;
