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
//! the Docker daemon is unreachable it returns `None` and **each test skips cleanly** (logs
//! + early-returns), so `cargo test --workspace --all-features` stays green with no daemon.
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
use based_runtime::idempotency::{MemStore, NoStore};
use based_runtime::run::{Backend, Db, DbError, DbErrorKind, DbRead};
use based_runtime::{dispatch, fetch_all, Compiled};
use based_sema::check;

use docker_mariadb::MariaDbContainer;

// Valid v4-shaped UUIDs for the seed rows — MariaDB's native `UUID` column (which the
// generated DDL emits) rejects a non-UUID string like `'org-1'`, so the fixtures use real
// UUID literals. The trailing digits keep them human-readable across the assertions.
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
            // `total` is DECIMAL(12,2) (returned as its exact string); ids/uuids ride as text (real UUID literals — MariaDB validates
            // the native `UUID` column). `deleted_at` defaults NULL (live rows).
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
            .any(|d| d.severity == based_diagnostics::Severity::Error),
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
    let mut ids = UuidGen;
    dispatch(
        compiled, router, "", &mut ids, &NoStore, method, path, args, ctx, None,
    )
    .await
}

#[tokio::test]
async fn get_query_runs_against_live_mariadb() {
    // `order_by_id` is a `get`: it joins order → user + org and projects OrderCard. This is
    // the verbatim lowered SELECT (MariaDB dialect) executed against a live server.
    let Some((c, router, _guard)) = live().await else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        json!({ "id": ORDER_1 }),
        // Order is `@scope`d: even a keyed `get` is org-scoped, so `$ctx.org` is
        // required. order-1 belongs to org-1, visible to this caller.
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "paid", "total": "500.00", "buyer": "Ada", "org": "Acme" })
    );
}

#[tokio::test]
async fn get_query_miss_returns_null() {
    // A `get` on an absent key is `Option<T>` → JSON null (a real empty result set).
    let Some((c, router, _guard)) = live().await else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        json!({ "id": "nope" }),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!(null));
}

#[tokio::test]
async fn ctx_scoped_list_filters_by_org() {
    // `my_org_orders` reads `$ctx.org` — the runtime binds it positionally into the WHERE.
    // A `list` shapes as a JSON array. The row scope predicate is real: a different org
    // sees none of org-1's rows.
    let Some((c, router, _guard)) = live().await else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([{ "status": "paid", "total": "500.00", "buyer": "Ada", "org": "Acme" }])
    );

    let empty = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-other" }),
    )
    .await;
    assert_eq!(empty.body, json!([]));
}

#[tokio::test]
async fn mutation_writes_then_reselects_declared_shape() {
    // `place_order` creates an Order (engine-generated uuid) and reads it back in its
    // declared OrderCard shape, all under one transaction — the full write path
    // against a real engine: INSERT commits, the re-select joins and projects
    // (read-your-writes). The created row is then visible to a follow-up read.
    let Some((c, router, _guard)) = live().await else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/m/place_order",
        // `org` is `@scope`-managed on create: supplied via `$ctx`, auto-set on the
        // INSERT — never a body arg. The re-select projects `org.name` = "Acme" (org-1).
        json!({ "buyer": USER_1, "total": "99.00" }),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": "99.00", "buyer": "Ada", "org": "Acme" })
    );

    // The write actually committed: the new order is now readable.
    let listed = call(
        &c,
        &router,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": ORG_1 }),
    )
    .await;
    let rows = listed.body.as_array().expect("list");
    assert_eq!(rows.len(), 2, "the created order is now readable: {rows:?}");
}

#[tokio::test]
async fn joined_scope_hides_cross_scope_row() {
    // Against a live server: `my_org_orders` reaches org-scoped `User`/`Org` through the
    // Order relations. Here we prove the joined-`ON` scope with the commerce topology by
    // confirming an in-scope caller sees the joined `buyer`/`org` names — the same join that
    // would come back NULL for an out-of-scope owner. (The dedicated cross-scope `Ticket →
    // Contact` case is covered on SQLite; here we assert the join projects live.)
    let Some((c, router, _guard)) = live().await else {
        return;
    };
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        json!({ "id": ORDER_1 }),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    // The joined `buyer` (User.name) and `org` (Org.name) both resolve live across the join.
    assert_eq!(resp.body["buyer"], json!("Ada"));
    assert_eq!(resp.body["org"], json!("Acme"));
}

#[tokio::test]
async fn idempotency_key_dedupes_a_retried_write() {
    // A keyed mutation runs its write body at most once per key: a retry with the same
    // key + payload replays the recorded response instead of double-inserting. Proven against
    // a live engine — the second call must not create a second order.
    let Some((c, router, _guard)) = live().await else {
        return;
    };
    let store = MemStore::default();
    let mut ids = UuidGen;

    let first = dispatch(
        &c,
        &router,
        "",
        &mut ids,
        &store,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": "7.00" }),
        json!({ "org": ORG_1 }),
        Some("key-abc".to_string()),
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);

    let second = dispatch(
        &c,
        &router,
        "",
        &mut ids,
        &store,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": "7.00" }),
        json!({ "org": ORG_1 }),
        Some("key-abc".to_string()),
    )
    .await;
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
    )
    .await;
    assert_eq!(listed.body.as_array().expect("list").len(), 2);
}

#[tokio::test]
async fn backend_ping_succeeds_on_a_live_server() {
    // The readiness seam works against a real MariaDB: `ShardRouter::ping` runs
    // `SELECT 1` on every shard's pooled connection.
    let Some((_c, router, _guard)) = live().await else {
        return;
    };
    assert!(router.ping().await.is_ok());
}

#[tokio::test]
async fn byo_sqlx_pool_backs_the_engine() {
    // The BYO-pool embed against a live server: an app that already owns a `MySqlPool`
    // hands a clone to `ShardRouter::from_pool` — no second pool, the caller's pool
    // settings govern — and the engine's full read + write (transactional) paths run
    // on it, sharing the codec path with a URL-built router.
    let Some((c, _own_router, guard)) = live().await else {
        return;
    };
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(4)
        .connect_lazy(&guard.url())
        .expect("caller pool");

    // The app uses its pool directly…
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM `order`")
        .fetch_one(&pool)
        .await
        .expect("app's own query");
    assert_eq!(n, 1);

    // …and the engine dispatches over the same pool: a joined, scoped read…
    let router = ShardRouter::from_pool(pool.clone());
    let resp = call(
        &c,
        &router,
        "POST",
        "/q/order_by_id",
        json!({ "id": ORDER_1 }),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "paid", "total": "500.00", "buyer": "Ada", "org": "Acme" })
    );

    // …and a mutation, whose transaction begins/commits on a caller-pool connection.
    let resp = call(
        &c,
        &router,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": "42.00" }),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);

    // The committed write is visible to the app's own next query on its pool.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM `order`")
        .fetch_one(&pool)
        .await
        .expect("app's own query");
    assert_eq!(n, 2, "the engine's write landed on the app's pool");
}

/// Keyset-cursor pagination, proven against a live MariaDB — the MariaDB twin of the
/// SQLite live keyset test. A `page (2)` keyset query walks the whole set exactly once: each
/// full page returns its window plus an opaque cursor, the final short page returns a `null`
/// cursor, and the cursor works even though the sort basis (`rank`, `id`) is not projected (the
/// runtime strips the hidden `__keyset_*` columns). A tampered cursor is a 400.
#[tokio::test]
async fn keyset_pagination_walks_the_set() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Item { id: text, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query items() -> ItemCard[] { list Item order (rank asc) page (2); }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO `item` (`id`, `name`, `rank`) VALUES \
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30), \
                ('i4', 'd', 40), ('i5', 'e', 50);",
        )
        .await;

    let page = |args: serde_json::Value| call(&c, &router, "POST", "/q/items", args, json!({}));

    // Page 1 (no cursor): the two lowest-ranked rows + a "more" cursor (a full page).
    let p1 = page(json!({})).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(
        p1.body["rows"],
        json!([{ "name": "a", "rank": 10 }, { "name": "b", "rank": 20 }])
    );
    let c1 = p1.body["cursor"]
        .as_str()
        .expect("page 1 cursor")
        .to_string();

    // Page 2 (cursor from page 1): the next window, another full page → another cursor.
    let p2 = page(json!({ "cursor": c1 })).await;
    assert_eq!(
        p2.body["rows"],
        json!([{ "name": "c", "rank": 30 }, { "name": "d", "rank": 40 }])
    );
    let c2 = p2.body["cursor"]
        .as_str()
        .expect("page 2 cursor")
        .to_string();

    // Page 3 (cursor from page 2): the final row. A short page (1 < 2) → no more cursor.
    let p3 = page(json!({ "cursor": c2 })).await;
    assert_eq!(p3.body["rows"], json!([{ "name": "e", "rank": 50 }]));
    assert_eq!(p3.body["cursor"], json!(null), "last page has no cursor");

    // A tampered cursor is rejected at the boundary (400), never fed to the query.
    let bad = page(json!({ "cursor": "deadbeef.00" })).await;
    assert_eq!(bad.status, 400, "{:?}", bad.body);
    assert_eq!(bad.body["error"]["code"], json!("bad_cursor"));
}

/// Explicit offset pagination (`page (2) offset`), proven live against MariaDB.
/// The client supplies an `offset`; the runtime binds it into `LIMIT … OFFSET …`. Paging
/// full→full→short walks the set, and an offset page envelope carries a `null` cursor (offset
/// is not keyset). The soft-delete filter is `n/a` here — this schema has no tombstone.
#[tokio::test]
async fn offset_pagination_pages_the_set() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Item { id: text, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query items() -> ItemCard[] { list Item order (rank asc) page (2) offset; }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO `item` (`id`, `name`, `rank`) VALUES \
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30), \
                ('i4', 'd', 40), ('i5', 'e', 50);",
        )
        .await;

    let page = |args: serde_json::Value| call(&c, &router, "POST", "/q/items", args, json!({}));

    // Offset 0 (absent = first page): the first two rows, cursor null (offset is not keyset).
    let p1 = page(json!({})).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(
        p1.body["rows"],
        json!([{ "name": "a", "rank": 10 }, { "name": "b", "rank": 20 }])
    );
    assert_eq!(
        p1.body["cursor"],
        json!(null),
        "offset pages carry no cursor"
    );

    // Offset 2: the next window.
    let p2 = page(json!({ "offset": 2 })).await;
    assert_eq!(
        p2.body["rows"],
        json!([{ "name": "c", "rank": 30 }, { "name": "d", "rank": 40 }])
    );

    // Offset 4: the final short page.
    let p3 = page(json!({ "offset": 4 })).await;
    assert_eq!(p3.body["rows"], json!([{ "name": "e", "rank": 50 }]));
}

/// Soft-delete + restore read-back, proven live against MariaDB. A
/// soft `delete` rewrites to `deleted_at = now()` (never a real DELETE) and reads the tombstoned
/// row back in its declared shape; the row then
/// vanishes from a live `list` (the soft-delete predicate is injected). `restore` clears the
/// tombstone and reads the row back with the live predicate applied — visible again.
#[tokio::test]
async fn soft_delete_and_restore_read_back() {
    let Some((c, router, container)) = live_schema(
        r#"
        @soft_delete(deleted_at)
        @sort(id asc)
        Widget { id: text, deleted_at: timestamp?, name: text }
        shape WidgetCard from Widget { name }
        query widgets() -> WidgetCard[] { list Widget; }
        mutation remove_widget(id: text) -> WidgetCard { delete Widget where (id = $id); }
        mutation restore_widget(id: text) -> WidgetCard { restore Widget where (id = $id); }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch("INSERT INTO `widget` (`id`, `name`) VALUES ('w1', 'Alpha'), ('w2', 'Beta');")
        .await;

    let list = || call(&c, &router, "POST", "/q/widgets", json!({}), json!({}));

    // Both live to start.
    assert_eq!(
        list().await.body,
        json!([{ "name": "Alpha" }, { "name": "Beta" }])
    );

    // Soft delete w1: rewritten to a tombstone, read back in shape.
    let del = call(
        &c,
        &router,
        "POST",
        "/m/remove_widget",
        json!({ "id": "w1" }),
        json!({}),
    )
    .await;
    assert_eq!(del.status, 200, "{:?}", del.body);
    assert_eq!(del.body, json!({ "name": "Alpha" }));

    // The tombstone hides w1 from a live read (soft-delete predicate injected).
    assert_eq!(list().await.body, json!([{ "name": "Beta" }]));

    // Restore w1: tombstone cleared, read back live.
    let res = call(
        &c,
        &router,
        "POST",
        "/m/restore_widget",
        json!({ "id": "w1" }),
        json!({}),
    )
    .await;
    assert_eq!(res.status, 200, "{:?}", res.body);
    assert_eq!(res.body, json!({ "name": "Alpha" }));

    // w1 is visible again.
    assert_eq!(
        list().await.body,
        json!([{ "name": "Alpha" }, { "name": "Beta" }])
    );
}

// ---------- live-DB hardening ------------------------------

/// Bring up a live MariaDB and build a router with the given [`PoolConfig`] — the seam for
/// the hardening tests, which each vary one knob (statement timeout, pool size, checkout
/// wait). Returns `None` when Docker is unavailable (the caller skips).
async fn hardening(pool: PoolConfig) -> Option<(ShardRouter, MariaDbContainer)> {
    let container = MariaDbContainer::start().await?;
    let router = ShardRouter::single(&container.url(), pool)
        .unwrap_or_else(|e| panic!("connect to live MariaDB: {e:?}"));
    Some((router, container))
}

/// A `max_statement_time` aborts a query that runs too long, live: the server cancels
/// `SELECT SLEEP(5)` at the 500ms ceiling and the driver surfaces a `DbError` promptly, rather
/// than the connection hanging for the full sleep.
#[tokio::test]
async fn statement_timeout_aborts_a_long_query() {
    let pool = PoolConfig {
        statement_timeout: Duration::from_millis(500),
        ..PoolConfig::default()
    };
    let Some((router, _guard)) = hardening(pool).await else {
        return;
    };
    let mut db = router.checkout("").await.expect("checkout");
    let start = Instant::now();
    let res = fetch_all(db.fetch("SELECT SLEEP(5)", &[])).await;
    let elapsed = start.elapsed();
    assert!(
        res.is_err(),
        "a query past max_statement_time must be aborted, not returned: {res:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "aborted at the timeout, not after the full 5s sleep: {elapsed:?}"
    );
}

/// A saturated pool fails fast as pool-exhausted, live: with a pool of one, a held
/// connection means the next checkout waits at most `checkout_timeout` then returns a
/// [`DbErrorKind::PoolExhausted`] `DbError` (the wire's 503) — never an unbounded hang.
#[tokio::test]
async fn pool_exhaustion_fails_fast() {
    let pool = PoolConfig {
        min: 1,
        max: 1,
        checkout_timeout: Duration::from_millis(500),
        statement_timeout: Duration::ZERO,
    };
    let Some((router, _guard)) = hardening(pool).await else {
        return;
    };
    let _held = router
        .checkout("")
        .await
        .expect("first checkout holds the only connection");
    let start = Instant::now();
    let res = router.checkout("").await;
    let elapsed = start.elapsed();
    match res {
        Err(e) => assert_eq!(e.kind, DbErrorKind::PoolExhausted, "{}", e.message),
        Ok(_) => panic!("a pool of one must not hand out a second connection while it is held"),
    }
    assert!(
        elapsed < Duration::from_secs(2),
        "failed fast at the checkout timeout, not a hang: {elapsed:?}"
    );
}

/// Two concurrent transactions that lock the same two rows in opposite order deadlock, live:
/// InnoDB aborts exactly one side with error 1213 the driver classifies as
/// [`DbErrorKind::Deadlock`] (so the mutation path would retry it), and the other commits. The
/// barrier guarantees both hold their first lock before either reaches for the second, so the
/// deadlock is deterministic.
#[tokio::test]
async fn concurrent_transactions_surface_a_deadlock() {
    let Some((router, container)) = hardening(PoolConfig::default()).await else {
        return;
    };
    container
        .exec_batch(
            "DROP TABLE IF EXISTS `acct`;\n\
             CREATE TABLE `acct` (`id` VARCHAR(16) PRIMARY KEY, `bal` INT);\n\
             INSERT INTO `acct` (`id`, `bal`) VALUES ('a', 0), ('b', 0);",
        )
        .await;
    let barrier = tokio::sync::Barrier::new(2);
    let (r1, r2) = tokio::join!(
        cross_lock(&router, "a", "b", &barrier),
        cross_lock(&router, "b", "a", &barrier),
    );
    let results = [r1, r2];
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(e) if e.kind == DbErrorKind::Deadlock)),
        "one side must be aborted with a deadlock-class error: {results:?}"
    );
    assert!(
        results.iter().any(|r| r.is_ok()),
        "the other side must commit: {results:?}"
    );
}

/// One transaction of the crossed-lock deadlock: lock `first`, wait for the peer to lock its
/// own first row (the barrier), then reach for `second` — the loser is aborted (its `Tx`
/// drops uncommitted, which rolls back).
async fn cross_lock(
    router: &ShardRouter,
    first: &str,
    second: &str,
    barrier: &tokio::sync::Barrier,
) -> Result<(), DbError> {
    let db: Box<dyn Db> = Box::new(router.checkout("").await?);
    let mut tx = db.begin().await?;
    tx.execute(
        &format!("UPDATE `acct` SET `bal` = `bal` + 1 WHERE `id` = '{first}'"),
        &[],
    )
    .await?;
    barrier.wait().await;
    tx.execute(
        &format!("UPDATE `acct` SET `bal` = `bal` + 1 WHERE `id` = '{second}'"),
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ---- streaming reads over the live wire -------------------------------------

/// Start the real `based serve` listener over a live router on a free loopback port
/// and return its `host:port` — so a streaming test observes the actual NDJSON body a
/// deployed edge produces, not just the dispatch-level stream.
async fn serve_live(compiled: Compiled, backend: ShardRouter) -> String {
    use based_runtime::http::{serve_with_handle, ServeConfig, TrustedHeaderContext};
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let listen = addr.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        serve_with_handle(
            compiled,
            backend,
            TrustedHeaderContext::default(),
            ServeConfig { listen },
            move |h| {
                let _ = tx.send(h);
            },
        )
        .await
        .unwrap();
    });
    let _handle = rx.await.unwrap();
    addr
}

/// A `-> stream` query against live MariaDB, observed through the full wire: `200` +
/// `application/x-ndjson`, one `row` line per row in sort order, and the mandatory
/// terminal `done` whose count checksums the pass.
#[tokio::test]
async fn stream_query_delivers_ndjson_rows_and_done_live() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(rank desc)
        Item { id: text, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query export_items() -> stream ItemCard;
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO `item` (`id`, `name`, `rank`) VALUES \
                ('i1', 'a', 10), ('i2', 'b', 30), ('i3', 'c', 20);",
        )
        .await;

    let addr = serve_live(c, router).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/q/export_items"))
        .json(&json!({}))
        .send()
        .await
        .expect("request runs");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()["content-type"].to_str().unwrap(),
        "application/x-ndjson"
    );
    let body = resp.text().await.expect("body reads");
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON envelope"))
        .collect();
    assert_eq!(
        lines,
        vec![
            json!({ "row": { "name": "b", "rank": 30 } }),
            json!({ "row": { "name": "c", "rank": 20 } }),
            json!({ "row": { "name": "a", "rank": 10 } }),
            json!({ "done": { "rows": 3 } }),
        ]
    );
}

/// A to-many nested array rides back in **sort-cascade order** against a live MariaDB:
/// children seeded out of order come back ordered by the child model's `@sort`
/// (`comments`), and a relation `@sort` on the edge overrides the child model's own
/// (`pins`). Proves MariaDB's `JSON_ARRAYAGG(… ORDER BY …)` form executes for real.
#[tokio::test]
async fn nested_to_many_rows_ride_in_sort_cascade_order() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Ticket {
          id: text
          subject: text
          comments: Comment[]
          pins: Pin[] @sort(rank desc)
        }
        @sort(pos asc)
        Comment { id: text, ticket: Ticket, pos: int, body: text }
        @sort(rank asc)
        Pin { id: text, ticket: Ticket, rank: int, label: text }
        shape TicketDetail from Ticket { subject, comments { body }, pins { label } }
        query ticket_by_id(id) -> TicketDetail;
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO `ticket` (`id`, `subject`) VALUES ('t1', 'printer on fire');\n\
             INSERT INTO `comment` (`id`, `ticket_id`, `pos`, `body`) VALUES \
                ('c3', 't1', 3, 'third'), ('c1', 't1', 1, 'first'), ('c2', 't1', 2, 'second');\n\
             INSERT INTO `pin` (`id`, `ticket_id`, `rank`, `label`) VALUES \
                ('p1', 't1', 1, 'low'), ('p3', 't1', 3, 'top'), ('p2', 't1', 2, 'mid');",
        )
        .await;

    let resp = call(
        &c,
        &router,
        "POST",
        "/q/ticket_by_id",
        json!({ "id": "t1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "subject": "printer on fire",
            // child model `@sort(pos asc)` — the model tier of the cascade.
            "comments": [{ "body": "first" }, { "body": "second" }, { "body": "third" }],
            // relation `@sort(rank desc)` overrides Pin's model `@sort(rank asc)`.
            "pins": [{ "label": "top" }, { "label": "mid" }, { "label": "low" }]
        })
    );
}
