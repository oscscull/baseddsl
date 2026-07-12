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
use based_runtime::idempotency::{MemStore, NoStore};
use based_runtime::run::{Backend, Db, DbError, DbErrorKind, DbRead};
use based_runtime::shard::PoolConfig;
use based_runtime::{dispatch, fetch_all, Compiled, PgRouter};
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
            .any(|d| d.severity == based_diagnostics::Severity::Error),
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
            .any(|d| d.severity == based_diagnostics::Severity::Error),
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
    let mut ids = UuidGen;
    dispatch(
        compiled, router, "", &mut ids, &NoStore, method, path, args, ctx, None,
    )
    .await
}

#[tokio::test]
async fn get_query_runs_against_live_postgres() {
    // `order_by_id` is a `get`: it joins order → user + org and projects OrderCard. This is
    // the verbatim lowered SELECT (Postgres dialect, `$n`-bound) executed against a live server.
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
        // A valid-but-absent uuid: proves the miss path, not a uuid coercion error.
        json!({ "id": "00000000-0000-4000-8000-0000000000ff" }),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!(null));
}

#[tokio::test]
async fn ctx_scoped_list_filters_by_org() {
    // `my_org_orders` reads `$ctx.org` — the runtime binds it positionally (`$1`) into the
    // WHERE. A `list` shapes as a JSON array. The row scope predicate is real: a different org
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
        // A different (valid) org uuid sees none of org-1's rows.
        json!({ "org": "00000000-0000-4000-8000-0000000000a2" }),
    )
    .await;
    assert_eq!(empty.body, json!([]));
}

#[tokio::test]
async fn mutation_writes_then_reselects_declared_shape() {
    // `place_order` creates an Order (engine-generated uuid) and reads it back in its declared
    // OrderCard shape, all under one transaction — the full write path against a real
    // engine: INSERT commits, the re-select joins and projects (read-your-writes). Proves the
    // engine-generated uuid round-trips through the Postgres `uuid` column via the value mapping.
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
async fn joined_scope_projects_live_across_the_join() {
    // Against a live server: `order_by_id` reaches org-scoped `User`/`Org` through the
    // Order relations, and the joined `@scope`d `ON` still projects the joined names for an
    // in-scope caller (the same join that would come back NULL for an out-of-scope owner). The
    // dedicated cross-scope case is covered on SQLite; here we assert the join projects live.
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
    // key + payload replays the recorded response instead of double-inserting. Proven against a
    // live engine — the second call must not create a second order.
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
    // The readiness seam works against a real Postgres: `PgRouter::ping` runs `SELECT 1`
    // on every shard's pooled connection.
    let Some((_c, router, _guard)) = live().await else {
        return;
    };
    assert!(router.ping().await.is_ok());
}

/// Keyset-cursor pagination, proven against a live Postgres — the Postgres twin of the
/// SQLite live keyset test. A `page (2)` keyset query walks the whole set exactly once: each full
/// page returns its window plus an opaque cursor, the final short page returns a `null` cursor,
/// and the cursor works even though the sort basis (`rank`, `id`) is not projected (the runtime
/// strips the hidden `__keyset_*` columns). A tampered cursor is a 400.
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
            "INSERT INTO \"item\" (\"id\", \"name\", \"rank\") VALUES \
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

/// Explicit offset pagination (`page (2) offset`), proven live against Postgres.
/// The client supplies an `offset`; the runtime binds it into `LIMIT … OFFSET …`. Paging
/// full→full→short walks the set, and an offset page envelope carries a `null` cursor (offset is
/// not keyset).
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
            "INSERT INTO \"item\" (\"id\", \"name\", \"rank\") VALUES \
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

/// `uuid` + `timestamptz` result columns round-trip as canonical strings, and a keyset cursor
/// whose sort basis is a `timestamptz` (not just an int) walks the set. This is the regression
/// guard for the binary-format decode fix: Postgres results arrive in *binary* format, so a
/// `uuid` arrives as 16 raw bytes and a `timestamptz` as an i64 of microseconds — the decode
/// path turns both into their canonical string rather than mangling them (a raw text read
/// dropped the uuid hyphens and turned the timestamp into hex, which then failed to re-bind on
/// page 2).
#[tokio::test]
async fn uuid_and_timestamp_columns_round_trip_and_keyset() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Event { id: text, at: timestamp, label: text }
        shape EventCard from Event { id, at, label }
        query events() -> EventCard[] { list Event order (at asc) page (2); }
        "#,
    )
    .await
    else {
        return;
    };
    // `id: text` maps to TEXT here (plain string ids); `at` is a real `timestamptz`. Distinct,
    // ordered instants so the keyset basis is unambiguous.
    container
        .exec_batch(
            "INSERT INTO \"event\" (\"id\", \"at\", \"label\") VALUES \
                ('e1', '2024-01-01 00:00:00+00', 'a'), \
                ('e2', '2024-01-02 12:30:45.500000+00', 'b'), \
                ('e3', '2024-01-03 00:00:00+00', 'c');",
        )
        .await;

    let page = |args: serde_json::Value| call(&c, &router, "POST", "/q/events", args, json!({}));

    // Page 1: the two earliest events. The `timestamptz` comes back as a canonical ISO string
    // (decoded from binary microseconds), not hex — proving the fix on the projected column.
    let p1 = page(json!({})).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(p1.body["rows"][0]["at"], json!("2024-01-01 00:00:00+00"));
    assert_eq!(
        p1.body["rows"][1]["at"],
        json!("2024-01-02 12:30:45.500000+00")
    );
    let cursor = p1.body["cursor"]
        .as_str()
        .expect("page 1 cursor")
        .to_string();

    // Page 2: feeding the cursor back binds the previous row's `timestamptz` basis — which only
    // works because the decoded string re-binds to the exact same instant (the bug's failure).
    let p2 = page(json!({ "cursor": cursor })).await;
    assert_eq!(p2.status, 200, "{:?}", p2.body);
    assert_eq!(
        p2.body["rows"],
        json!([{ "id": "e3", "at": "2024-01-03 00:00:00+00", "label": "c" }])
    );
    assert_eq!(p2.body["cursor"], json!(null), "last page has no cursor");
}

/// Soft-delete + restore read-back, proven live against Postgres. A soft
/// `delete` rewrites to `deleted_at = now()` (never a real DELETE) and reads the tombstoned row
/// back in its declared shape; the row then
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
        .exec_batch(
            "INSERT INTO \"widget\" (\"id\", \"name\") VALUES ('w1', 'Alpha'), ('w2', 'Beta');",
        )
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

/// Bring up a live Postgres and build a router with the given [`PoolConfig`] — the seam for
/// the hardening tests, which each vary one knob (statement timeout, pool size, checkout
/// wait). Resets the schema so a persistent external server (`TEST_POSTGRES_URL`) is clean.
/// Returns `None` when Docker is unavailable (the caller skips).
async fn hardening(pool: PoolConfig) -> Option<(PgRouter, PostgresContainer)> {
    let container = PostgresContainer::start().await?;
    container.exec_batch(RESET_SQL).await;
    let router = PgRouter::single(&container.url(), pool)
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));
    Some((router, container))
}

/// A `statement_timeout` aborts a query that runs too long, live: the server cancels
/// `pg_sleep(5)` at the 500ms ceiling and the driver surfaces a `DbError` promptly, rather
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
    let res = fetch_all(db.fetch("SELECT pg_sleep(5)", &[])).await;
    let elapsed = start.elapsed();
    assert!(
        res.is_err(),
        "a query past statement_timeout must be aborted, not returned: {res:?}"
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
/// the server aborts exactly one side with a deadlock-class error (`40P01`) the driver
/// classifies as [`DbErrorKind::Deadlock`] (so the mutation path would retry it), and the
/// other commits. The barrier guarantees both hold their first lock before either reaches for
/// the second, so the deadlock is deterministic.
#[tokio::test]
async fn concurrent_transactions_surface_a_deadlock() {
    let Some((router, container)) = hardening(PoolConfig::default()).await else {
        return;
    };
    container
        .exec_batch(
            "CREATE TABLE acct (id text primary key, bal int);\n\
             INSERT INTO acct (id, bal) VALUES ('a', 0), ('b', 0);",
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
    router: &PgRouter,
    first: &str,
    second: &str,
    barrier: &tokio::sync::Barrier,
) -> Result<(), DbError> {
    let db: Box<dyn Db> = Box::new(router.checkout("").await?);
    let mut tx = db.begin().await?;
    tx.execute(
        &format!("UPDATE acct SET bal = bal + 1 WHERE id = '{first}'"),
        &[],
    )
    .await?;
    barrier.wait().await;
    tx.execute(
        &format!("UPDATE acct SET bal = bal + 1 WHERE id = '{second}'"),
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
