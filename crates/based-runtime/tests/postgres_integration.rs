//! End-to-end integration against a **real** Postgres server, over Docker (A3/D38).
//!
//! The Postgres twin of `mariadb_integration.rs`: it loads the *actual* commerce schema (the
//! same discover → parse → check front end the CLI uses), lowers it for **`Dialect::Postgres`**
//! (so the DML binds `$n`, D29 — *not* the manifest's `mariadb`), creates its tables from the
//! *generated* Postgres DDL (`sql::ddl(_, Dialect::Postgres)`), and drives real requests
//! through `serve::dispatch` against the concrete `PostgresDb` driver checked out of a live
//! `PgRouter`. What runs is the *verbatim* codegen-lowered Postgres SQL — bound positionally
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

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::id::UuidGen;
use based_runtime::idempotency::{MemStore, NoStore};
use based_runtime::run::Backend;
use based_runtime::shard::PoolConfig;
use based_runtime::{dispatch, pg_connect, Compiled, PgRouter};
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
fn live() -> Option<(Compiled, PgRouter, PostgresContainer)> {
    let container = PostgresContainer::start()?;

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
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "commerce must check clean: {diags:?}"
    );
    let compiled = Compiled::from_checked(schema, decls, Dialect::Postgres);
    assert_eq!(compiled.dialect, Dialect::Postgres);

    let router = PgRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));

    // Create every commerce table from the *generated* Postgres DDL (not a hand copy), then
    // seed fixtures — so this suite exercises the whole `based gen sql` artifact (DDL + DML).
    // The DDL + seed run through a one-shot client (`batch_execute` handles a multi-statement
    // script; the `postgres` driver's `execute` is single-statement).
    let ddl = sql::ddl(&compiled.schema, Dialect::Postgres);
    let mut client = pg_connect(&container.url()).expect("setup client");
    client
        .batch_execute(&ddl)
        .expect("create tables from generated DDL");
    client
        .batch_execute(&format!(
            // `total` is BIGINT; ids/uuids ride as text literals Postgres coerces into `uuid`;
            // `deleted_at` defaults NULL (live rows).
            "INSERT INTO \"org\" (\"id\", \"name\", \"slug\") VALUES ('{ORG_1}', 'Acme', 'acme');\n\
             INSERT INTO \"user\" (\"id\", \"email\", \"name\") VALUES ('{USER_1}', 'a@x.com', 'Ada');\n\
             INSERT INTO \"order\" (\"id\", \"org_id\", \"placed_by_id\", \"status\", \"total\")\n\
                 VALUES ('{ORDER_1}', '{ORG_1}', '{USER_1}', 'paid', 500);"
        ))
        .expect("seed fixtures");

    Some((compiled, router, container))
}

/// Run one request through the real dispatch core against a connection checked out of the
/// live router — the exact path `based serve` uses, minus the socket.
fn call(
    compiled: &Compiled,
    router: &PgRouter,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
) -> based_runtime::WireResponse {
    let mut db = router.checkout("").expect("checkout");
    let mut ids = UuidGen;
    dispatch(
        compiled, &mut db, &mut ids, &NoStore, method, path, args, ctx, None,
    )
}

#[test]
fn get_query_runs_against_live_postgres() {
    // `order_by_id` is a `get`: it joins order → user + org and projects OrderCard. This is
    // the verbatim lowered SELECT (Postgres dialect, `$n`-bound) executed against a live server.
    let Some((c, router, _guard)) = live() else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        json!({ "id": ORDER_1 }),
        // Order is `@scope`d (D32): even a keyed `get` is org-scoped, so `$ctx.org` is
        // required. order-1 belongs to org-1, visible to this caller.
        json!({ "org": ORG_1 }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "paid", "total": 500, "buyer": "Ada", "org": "Acme" })
    );
}

#[test]
fn get_query_miss_returns_null() {
    // A `get` on an absent key is `Option<T>` → JSON null (a real empty result set).
    let Some((c, router, _guard)) = live() else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        // A valid-but-absent uuid: proves the miss path, not a uuid coercion error.
        json!({ "id": "00000000-0000-4000-8000-0000000000ff" }),
        json!({ "org": ORG_1 }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!(null));
}

#[test]
fn ctx_scoped_list_filters_by_org() {
    // `my_org_orders` reads `$ctx.org` — the runtime binds it positionally (`$1`) into the
    // WHERE. A `list` shapes as a JSON array. The row scope predicate is real: a different org
    // sees none of org-1's rows.
    let Some((c, router, _guard)) = live() else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": ORG_1 }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([{ "status": "paid", "total": 500, "buyer": "Ada", "org": "Acme" }])
    );

    let empty = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        // A different (valid) org uuid sees none of org-1's rows.
        json!({ "org": "00000000-0000-4000-8000-0000000000a2" }),
    );
    assert_eq!(empty.body, json!([]));
}

#[test]
fn mutation_writes_then_reselects_declared_shape() {
    // `place_order` creates an Order (engine-generated uuid) and reads it back in its declared
    // OrderCard shape (D12), all under one transaction — the full write path against a real
    // engine: INSERT commits, the re-select joins and projects (read-your-writes). Proves the
    // engine-generated uuid round-trips through the Postgres `uuid` column via the value mapping.
    let Some((c, router, _guard)) = live() else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/m/place_order",
        // `org` is `@scope`-managed on create (D32): supplied via `$ctx`, auto-set on the
        // INSERT — never a body arg. The re-select projects `org.name` = "Acme" (org-1).
        json!({ "buyer": USER_1, "total": 99 }),
        json!({ "org": ORG_1 }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": 99, "buyer": "Ada", "org": "Acme" })
    );

    // The write actually committed: the new order is now readable.
    let listed = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": ORG_1 }),
    );
    let rows = listed.body.as_array().expect("list");
    assert_eq!(rows.len(), 2, "the created order is now readable: {rows:?}");
}

#[test]
fn joined_scope_projects_live_across_the_join() {
    // D34 against a live server: `order_by_id` reaches org-scoped `User`/`Org` through the
    // Order relations, and the joined `@scope`d `ON` still projects the joined names for an
    // in-scope caller (the same join that would come back NULL for an out-of-scope owner). The
    // dedicated cross-scope case is covered on SQLite; here we assert the join projects live.
    let Some((c, router, _guard)) = live() else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        json!({ "id": ORDER_1 }),
        json!({ "org": ORG_1 }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    // The joined `buyer` (User.name) and `org` (Org.name) both resolve live across the join.
    assert_eq!(resp.body["buyer"], json!("Ada"));
    assert_eq!(resp.body["org"], json!("Acme"));
}

#[test]
fn idempotency_key_dedupes_a_retried_write() {
    // A keyed mutation runs its write body at most once per key (D25): a retry with the same
    // key + payload replays the recorded response instead of double-inserting. Proven against a
    // live engine — the second call must not create a second order.
    let Some((c, router, _guard)) = live() else {
        return;
    };
    let store = MemStore::default();
    let mut ids = UuidGen;

    let mut first_db = router.checkout("").expect("checkout");
    let first = dispatch(
        &c,
        &mut first_db,
        &mut ids,
        &store,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": 7 }),
        json!({ "org": ORG_1 }),
        Some("key-abc".to_string()),
    );
    assert_eq!(first.status, 200, "{:?}", first.body);
    drop(first_db);

    let mut second_db = router.checkout("").expect("checkout");
    let second = dispatch(
        &c,
        &mut second_db,
        &mut ids,
        &store,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": 7 }),
        json!({ "org": ORG_1 }),
        Some("key-abc".to_string()),
    );
    drop(second_db);
    // The retry replays the first response — same body, no second insert.
    assert_eq!(second.status, 200, "{:?}", second.body);
    assert_eq!(first.body, second.body);

    // Exactly one order was created for this key (plus the seeded order-1) → 2 total.
    let listed = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": ORG_1 }),
    );
    assert_eq!(listed.body.as_array().expect("list").len(), 2);
}

#[test]
fn backend_ping_succeeds_on_a_live_server() {
    // The readiness seam (D26) works against a real Postgres: `PgRouter::ping` runs `SELECT 1`
    // on every shard's pooled connection.
    let Some((_c, router, _guard)) = live() else {
        return;
    };
    assert!(router.ping().is_ok());
}
