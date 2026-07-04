//! End-to-end integration against a **real** engine (SQLite), no mock (D27).
//!
//! Every other runtime test drives the plan → run → shape path against a `MockDb` that
//! returns canned rows — so it proves the *binding* is right but never that the emitted
//! SQL actually executes. This test closes that gap: it loads the real commerce schema
//! (`Compiled::load`), seeds an in-memory SQLite database, and dispatches real requests
//! through `serve::dispatch` against the concrete `SqliteDb`/`SqliteBackend`. What runs is
//! the *verbatim* codegen-lowered SQL (`based gen sql`), bound positionally by the runtime
//! — so a passing test means the whole engine works against a genuine database, and that
//! the `Db`/`Backend`/`ping` seams are real, not just compile-verified.
//!
//! It needs no infra (bundled in-memory SQLite), so it runs in CI like any unit test.
//!
//! SQLite accepts the runtime's DML as-is (backtick identifiers, `= TRUE`, `IS NULL`,
//! joins, `LIMIT`, positional `?`); the only dialect gap is DDL (`KEY`/`UNIQUE KEY` inline
//! index syntax), which is *test setup* here — the schema is created with SQLite-shaped
//! DDL below. Full SQLite DDL codegen (a `Dialect` variant) is a separate slice (D27).

#![cfg(feature = "sqlite")]

use std::path::PathBuf;

use serde_json::json;

use based_runtime::idempotency::NoStore;
use based_runtime::run::Backend;
use based_runtime::{dispatch, Compiled, SeqIdGen, SqliteBackend};

/// Load the real commerce example — the same front end (discover → parse → check) +
/// codegen lowering the CLI uses, so the SQL executed here is `based gen sql`'s output.
fn commerce() -> Compiled {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/examples/commerce")
        .canonicalize()
        .expect("commerce example dir");
    Compiled::load(&root).unwrap_or_else(|e| panic!("commerce did not load: {e:?}"))
}

/// Seed an in-memory SQLite database with the tables the commerce queries touch, plus a
/// couple of rows. The DDL is hand-shaped for SQLite (no inline `KEY`) since dialect-aware
/// DDL codegen is out of scope for this slice; the *runtime* SQL under test is unchanged.
fn seeded_backend() -> SqliteBackend {
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    backend
        .execute_batch(
            r#"
            CREATE TABLE `org` (
                `id` TEXT NOT NULL PRIMARY KEY,
                `deleted_at` TEXT NULL,
                `name` TEXT NOT NULL,
                `slug` TEXT NOT NULL
            );
            CREATE TABLE `user` (
                `id` TEXT NOT NULL PRIMARY KEY,
                `deleted_at` TEXT NULL,
                `email` TEXT NOT NULL,
                `name` TEXT NOT NULL,
                `invited_by_id` TEXT NULL
            );
            CREATE TABLE `order` (
                `id` TEXT NOT NULL PRIMARY KEY,
                `deleted_at` TEXT NULL,
                `org_id` TEXT NOT NULL,
                `placed_by_id` TEXT NOT NULL,
                `fulfilled_by_id` TEXT NULL,
                `status` TEXT NOT NULL DEFAULT 'pending',
                `total` INTEGER NOT NULL,
                `placed_at` TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            INSERT INTO `org` (`id`, `name`, `slug`) VALUES ('org-1', 'Acme', 'acme');
            INSERT INTO `user` (`id`, `email`, `name`) VALUES ('user-1', 'a@x.com', 'Ada');
            INSERT INTO `order` (`id`, `org_id`, `placed_by_id`, `status`, `total`)
                VALUES ('order-1', 'org-1', 'user-1', 'paid', 500);
            "#,
        )
        .expect("seed schema + fixtures");
    backend
}

/// Run one request through the real dispatch core against a checked-out `SqliteDb`.
fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
) -> based_runtime::WireResponse {
    let mut db = backend.checkout("").expect("checkout");
    let mut ids = SeqIdGen::default();
    dispatch(
        compiled,
        db.as_mut(),
        &mut ids,
        &NoStore,
        method,
        path,
        args,
        ctx,
        None,
    )
}

#[test]
fn get_query_runs_against_real_sqlite() {
    // `order_by_id` is a `get`: it joins order → user + org and projects the OrderCard
    // shape. This is the verbatim lowered SELECT executed against a live SQLite row.
    let c = commerce();
    let backend = seeded_backend();
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "order-1" }),
        json!({}),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "paid", "total": 500, "buyer": "Ada", "org": "Acme" })
    );
}

#[test]
fn get_query_misses_return_null() {
    // A `get` on an absent key is `Option<T>` → JSON null (the envelope, realized by a
    // real empty result set, not a canned one).
    let c = commerce();
    let backend = seeded_backend();
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "nope" }),
        json!({}),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!(null));
}

#[test]
fn ctx_scoped_list_query_binds_context() {
    // `my_org_orders` reads `$ctx.org` — the server supplies it out of band, and the
    // runtime binds it positionally into the WHERE. A `list` shapes as a JSON array.
    let c = commerce();
    let backend = seeded_backend();
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-1" }),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([{ "status": "paid", "total": 500, "buyer": "Ada", "org": "Acme" }])
    );

    // A different org sees none of org-1's rows — the injected scope predicate is real.
    let empty = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-other" }),
    );
    assert_eq!(empty.body, json!([]));
}

#[test]
fn mutation_writes_then_reselects_declared_shape() {
    // `place_order` creates an Order (engine-generated id) and reads it back in its
    // declared OrderCard shape (D12), all under one transaction — the full write path
    // against a real engine: INSERT executes, the re-select joins and projects.
    let c = commerce();
    let backend = seeded_backend();
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/place_order",
        json!({ "org": "org-1", "buyer": "user-1", "total": 99 }),
        json!({}),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    // The response is the created row in its declared shape (status defaults to 'pending').
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": 99, "buyer": "Ada", "org": "Acme" })
    );

    // The write actually committed: the new order is now visible to a read.
    let listed = call(
        &c,
        &backend,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-1" }),
    );
    let rows = listed.body.as_array().expect("list");
    assert_eq!(
        rows.len(),
        2,
        "the created order is now readable: {:?}",
        rows
    );
}

#[test]
fn bad_arg_is_a_400_before_sql() {
    // A mistyped arg is a boundary error caught before any SQL touches SQLite.
    let c = commerce();
    let backend = seeded_backend();
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/place_order",
        json!({ "org": "org-1", "buyer": "user-1", "total": "not-an-int" }),
        json!({}),
    );
    assert_eq!(resp.status, 400, "{:?}", resp.body);
    assert_eq!(resp.body["error"]["code"], json!("bad_arg"));
}

#[test]
fn backend_ping_succeeds_on_a_live_db() {
    // The readiness seam (D26) works against a real engine: `SELECT 1` round-trips.
    assert!(seeded_backend().ping().is_ok());
}
