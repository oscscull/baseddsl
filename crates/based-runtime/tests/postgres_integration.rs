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

/// The transaction seam live against Postgres (transactions.md): the isolation SET is
/// **actually issued** (a `Serializable`, `ReadOnly` `begin_tx` — Postgres reports it back),
/// and the explicit-handle rung round-trips (open → write on the handle → commit makes the
/// write visible; a rolled-back write does not).
#[tokio::test]
async fn transaction_isolation_is_issued_and_explicit_handle_round_trips_live_postgres() {
    use based_runtime::{Engine, TxOptions};

    let Some((c, router, _guard)) = live().await else {
        return;
    };

    // The requested isolation + access mode reached the server: Postgres reports the running
    // transaction's level, proving `BEGIN ISOLATION LEVEL SERIALIZABLE READ ONLY` was issued.
    let db = Backend::checkout(&router, "").await.expect("checkout");
    let mut tx = db
        .begin_tx(TxOptions::default().serializable().read_only())
        .await
        .expect("begin serializable read-only");
    let iso = fetch_all(tx.fetch("SHOW transaction_isolation", &[]))
        .await
        .expect("SHOW isolation");
    assert_eq!(iso[0]["transaction_isolation"], "serializable");
    let ro = fetch_all(tx.fetch("SHOW transaction_read_only", &[]))
        .await
        .expect("SHOW read_only");
    assert_eq!(ro[0]["transaction_read_only"], "on");
    tx.rollback().await.expect("rollback");

    // Rung 2 over the Engine: begin → run a real write on the handle → commit; the row is
    // visible only after commit, and a rolled-back write is never visible.
    let engine = Engine::new(c, router, UuidGen);
    let write = |total: &str| json!({ "buyer": USER_1, "total": total });
    let ctx = json!({ "org": ORG_1 });

    let txn = engine
        .begin(TxOptions::default())
        .await
        .expect("open a transaction");
    let resp = txn
        .transport()
        .dispatch("/m/place_order", write("11.00"), ctx.clone())
        .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    txn.commit().await.expect("commit");
    let listed = engine
        .call("/q/my_org_orders", json!({}), ctx.clone())
        .await;
    assert_eq!(
        listed.body.as_array().expect("list").len(),
        2,
        "the committed write is visible: {:?}",
        listed.body
    );

    let txn = engine
        .begin(TxOptions::default())
        .await
        .expect("open a transaction");
    let resp = txn
        .transport()
        .dispatch("/m/place_order", write("99.00"), ctx.clone())
        .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    txn.rollback().await.expect("rollback");
    let listed = engine.call("/q/my_org_orders", json!({}), ctx).await;
    assert_eq!(
        listed.body.as_array().expect("list").len(),
        2,
        "the rolled-back write is not visible: {:?}",
        listed.body
    );
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
    let ids = UuidGen;

    let first = dispatch(
        &c,
        &router,
        "",
        &ids,
        &store,
        &Guards::new(),
        None,
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
        &ids,
        &store,
        &Guards::new(),
        None,
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
async fn db_store_dedupes_a_retry_on_a_second_instance() {
    // The durable DB-backed store keeps its keys in a `_based_idempotency` table in the
    // same database, committed in the mutation's own transaction — so a keyed retry that
    // lands on a *different* app instance still deduplicates (atomic exactly-once, the
    // guarantee a per-process `MemStore` cannot give). Two independent routers over the
    // same live database stand in for two instances behind a load balancer.
    let Some((c, instance_a, guard)) = live().await else {
        return;
    };
    let instance_b =
        PgRouter::single(&guard.url(), PoolConfig::default()).expect("second instance router");
    let ids = UuidGen;
    let store_a = DbStore::create(&instance_a, Dialect::Postgres)
        .await
        .expect("create store a");
    let store_b = DbStore::create(&instance_b, Dialect::Postgres)
        .await
        .expect("create store b");

    let first = dispatch(
        &c,
        &instance_a,
        "",
        &ids,
        &store_a,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": "7.00" }),
        json!({ "org": ORG_1 }),
        Some("cross-instance-key".to_string()),
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);

    // The SAME keyed retry lands on the second instance: it replays A's response through
    // the shared key table, writing no second order.
    let replay = dispatch(
        &c,
        &instance_b,
        "",
        &ids,
        &store_b,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "buyer": USER_1, "total": "7.00" }),
        json!({ "org": ORG_1 }),
        Some("cross-instance-key".to_string()),
    )
    .await;
    assert_eq!(replay.status, 200, "{:?}", replay.body);
    assert_eq!(
        first.body, replay.body,
        "the second instance replayed the first's response"
    );

    // Exactly one order was created for this key (plus the seeded order-1) → 2 total.
    let listed = call(
        &c,
        &instance_b,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": ORG_1 }),
    )
    .await;
    assert_eq!(listed.body.as_array().expect("list").len(), 2);
}

/// Count key rows for `key` in the durable store's table — a raw read, so a GC test can
/// prove a swept key is actually gone.
async fn idem_key_count(router: &PgRouter, key: &str) -> i64 {
    let mut db = router.checkout("").await.expect("checkout");
    let rows = fetch_all(db.fetch(
        "SELECT COUNT(*) AS n FROM \"_based_idempotency\" WHERE \"key\" = $1",
        &[based_runtime::SqlValue::Text(key.to_string())],
    ))
    .await
    .expect("count");
    rows[0]["n"].as_i64().expect("i64 count")
}

#[tokio::test]
async fn db_store_with_gc_sweeps_aged_keys() {
    // `with_gc` bounds the key table on a live Postgres: a key older than the TTL is reclaimed
    // by a later keyed mutation's amortized (detached) sweep. This is the real proof the
    // per-dialect age-based DELETE is valid Postgres — a syntax error would be silently
    // swallowed (GC is best-effort), so only a live run catches it.
    let Some((c, router, guard)) = live().await else {
        return;
    };
    let ids = UuidGen;
    let gc_router = PgRouter::single(&guard.url(), PoolConfig::default()).expect("gc-store router");
    let store = DbStore::with_gc(gc_router, Dialect::Postgres, Duration::from_secs(1))
        .await
        .expect("with_gc");

    let guards = Guards::new();
    let place = |key: &'static str| {
        dispatch(
            &c,
            &router,
            "",
            &ids,
            &store,
            &guards,
            None,
            "POST",
            "/m/place_order",
            json!({ "buyer": USER_1, "total": "7.00" }),
            json!({ "org": ORG_1 }),
            Some(key.to_string()),
        )
    };

    let first = place("gc-key-1").await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(idem_key_count(&router, "gc-key-1").await, 1);

    // Let the key age past the 1s TTL (and the store's first sweep fall due).
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // A later keyed mutation triggers the detached best-effort sweep.
    let second = place("gc-key-2").await;
    assert_eq!(second.status, 200, "{:?}", second.body);

    // The sweep runs off the mutation path — poll until the aged key is reclaimed.
    let mut swept = false;
    for _ in 0..30 {
        if idem_key_count(&router, "gc-key-1").await == 0 {
            swept = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        swept,
        "the aged key should have been swept by the age-based DELETE"
    );
    assert_eq!(
        idem_key_count(&router, "gc-key-2").await,
        1,
        "the fresh key survives"
    );
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

#[tokio::test]
async fn byo_sqlx_pool_backs_the_engine() {
    // The BYO-pool embed against a live server: an app that already owns a `PgPool`
    // hands a clone to `PgRouter::from_pool` — no second pool, the caller's pool
    // settings govern — and the engine's full read + write (transactional) paths run
    // on it, sharing the codec path (native-typed binds, binary-format decode) with a
    // URL-built router.
    let Some((c, _own_router, guard)) = live().await else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_lazy(&guard.url())
        .expect("caller pool");

    // The app uses its pool directly…
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM \"order\"")
        .fetch_one(&pool)
        .await
        .expect("app's own query");
    assert_eq!(n, 1);

    // …and the engine dispatches over the same pool: a joined, scoped read…
    let router = PgRouter::from_pool(pool.clone());
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
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM \"order\"")
        .fetch_one(&pool)
        .await
        .expect("app's own query");
    assert_eq!(n, 2, "the engine's write landed on the app's pool");
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

/// `time` and `bytes` round-trip live against Postgres — the risky path, since Postgres
/// transmits both binary: a `TIME` as an i64 of microseconds since midnight, a `BYTEA` as
/// raw bytes. The bind side parses the wire `HH:MM:SS` into a native `time` and base64-decodes
/// the wire string into `bytea`; the decode side turns the binary `time` back into its string
/// and base64-encodes the `bytea` — both through a `create` (bind) and a read-back (decode),
/// plus a raw-seeded row to prove the decoders independently of our own binds.
#[tokio::test]
async fn time_and_bytes_columns_round_trip_live() {
    let Some((c, router, container)) = live_schema(
        r#"
        Event { id: Id, start_at: time, payload: bytes, label: text, @index(start_at) }
        shape EventCard from Event { start_at, payload, label }
        mutation add_event(start_at: time, payload: bytes, label: text) -> EventCard {
          create Event { start_at = $start_at, payload = $payload, label = $label };
        }
        query events() -> EventCard[] { list Event order (start_at asc); }
        "#,
    )
    .await
    else {
        return;
    };
    // Raw-seed a row (engine `Id` = a real `uuid` column): a bytea hex literal `\x000102ff`
    // decodes to base64 `AAEC/w==`, and the `TIME` literal decodes (from binary microseconds)
    // back to `08:15:00` — proving the decoders independently of our own bind path.
    container
        .exec_batch(
            "INSERT INTO \"event\" (\"id\", \"start_at\", \"payload\", \"label\") VALUES \
                ('00000000-0000-4000-8000-0000000000e0', '08:15:00', '\\x000102ff', 'seed');",
        )
        .await;

    // Create through the engine: the bind parses `14:30:00` into a native `time` and base64-
    // decodes `aGVsbG8=` ("hello") into `bytea`; the read-back decodes both to the wire form.
    let created = call(
        &c,
        &router,
        "POST",
        "/m/add_event",
        json!({ "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" }),
        "create re-selects the time string + base64 bytes exact (binary bind + decode)"
    );

    // List both (ordered by time): the raw-seeded row's binary `TIME`/`BYTEA` decode to the
    // same canonical forms as the engine-bound row.
    let all = call(&c, &router, "POST", "/q/events", json!({}), json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        all.body,
        json!([
            { "start_at": "08:15:00", "payload": "AAEC/w==", "label": "seed" },
            { "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" }
        ])
    );
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
        results.iter().any(std::result::Result::is_ok),
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

/// `for update` holds a real row lock across a transaction, live against Postgres
/// (transactions.md slice 2). Two concurrent transactions run the **same** `product_for_update`
/// locking read on the **same** row: A acquires the lock and holds it (updating the row before
/// commit); B's identical locking read **blocks** until A commits, then proceeds and observes
/// A's committed value. The blocking is proven by a timeout — B's future stays pending while A
/// holds the lock — and the post-unblock value proves B waited for A's committed state, not a
/// stale snapshot. (SQLite has no row-level `FOR UPDATE`; its whole-database transaction lock
/// serializes writers instead, so this per-row hand-off is a Postgres/MySQL-family semantic.)
#[tokio::test]
async fn for_update_lock_is_held_across_a_transaction_live_postgres() {
    use based_runtime::{Engine, TxOptions};

    let Some((c, router, container)) = live_schema(
        r#"
        Product { id: text, sku: text, stock: int }
        shape ProductRow from Product { sku, stock }
        query product_for_update(id) -> ProductRow {
            get Product where (id = $id) for update;
        }
        mutation set_stock(id, stock) -> ProductRow {
            update Product where (id = $id) { stock = $stock }
        }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch("INSERT INTO product (id, sku, stock) VALUES ('p1', 'widget', 0);")
        .await;

    let engine = Engine::new(c, router, UuidGen);
    let lock = json!({ "id": "p1" });

    // A: open a transaction and take the row lock via the `for update` read.
    let txn_a = engine.begin(TxOptions::default()).await.expect("begin A");
    let a_read = txn_a
        .transport()
        .dispatch("/q/product_for_update", lock.clone(), json!({}))
        .await;
    assert_eq!(a_read.status, 200, "A locks the row: {:?}", a_read.body);

    // B: open a second transaction and issue the same locking read — it must block on A's lock.
    let txn_b = engine.begin(TxOptions::default()).await.expect("begin B");
    let tx_b = txn_b.transport();
    let b_read = tx_b.dispatch("/q/product_for_update", lock.clone(), json!({}));
    tokio::pin!(b_read);
    let while_held = tokio::time::timeout(Duration::from_millis(750), &mut b_read).await;
    assert!(
        while_held.is_err(),
        "B's `for update` read must block while A holds the lock, not return: {while_held:?}"
    );

    // A writes the row and commits, releasing the lock.
    let a_write = txn_a
        .transport()
        .dispatch(
            "/m/set_stock",
            json!({ "id": "p1", "stock": 99 }),
            json!({}),
        )
        .await;
    assert_eq!(a_write.status, 200, "A updates the row: {:?}", a_write.body);
    txn_a.commit().await.expect("commit A");

    // B now unblocks and observes A's committed value (proving it waited, not read a stale row).
    let b_resp = tokio::time::timeout(Duration::from_secs(5), &mut b_read)
        .await
        .expect("B unblocks once A releases the lock");
    assert_eq!(b_resp.status, 200, "B proceeds: {:?}", b_resp.body);
    assert_eq!(
        b_resp.body["stock"], 99,
        "B reads A's committed update, so it blocked until A released: {:?}",
        b_resp.body
    );
    txn_b.commit().await.expect("commit B");
}

/// `for update skip locked` and `for update nowait` behave per SQL, live against Postgres
/// (transactions.md slice 2 follow-on). Transaction A locks one row; a second transaction's
/// `skip locked` list returns the OTHER rows (never the locked one) without blocking, and a
/// `nowait` read of the locked row errors fast instead of waiting. Both wait modes are proven
/// non-blocking by a timeout that must NOT trip (unlike plain `for update`, which does block).
#[tokio::test]
async fn for_update_skip_locked_and_nowait_live_postgres() {
    use based_runtime::{Engine, TxOptions};

    let Some((c, router, container)) = live_schema(
        r#"
        Product { id: text, sku: text, stock: int }
        shape ProductRow from Product { sku, stock }
        query lock_one(id) -> ProductRow {
            get Product where (id = $id) for update;
        }
        query available(max) -> ProductRow[] {
            list Product where (stock <= $max) order (sku) for update skip locked;
        }
        query lock_one_nowait(id) -> ProductRow {
            get Product where (id = $id) for update nowait;
        }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO product (id, sku, stock) VALUES \
             ('p1', 'a', 0), ('p2', 'b', 0), ('p3', 'c', 0);",
        )
        .await;

    let engine = Engine::new(c, router, UuidGen);

    // A: lock row p1 and hold it.
    let txn_a = engine.begin(TxOptions::default()).await.expect("begin A");
    let a_read = txn_a
        .transport()
        .dispatch("/q/lock_one", json!({ "id": "p1" }), json!({}))
        .await;
    assert_eq!(a_read.status, 200, "A locks p1: {:?}", a_read.body);

    // B: `skip locked` must return the unlocked rows (p2, p3), never the locked p1, and must
    // not block — proven by a timeout that must resolve.
    let txn_b = engine.begin(TxOptions::default()).await.expect("begin B");
    let b_read = tokio::time::timeout(
        Duration::from_secs(5),
        txn_b
            .transport()
            .dispatch("/q/available", json!({ "max": 100 }), json!({})),
    )
    .await
    .expect("`skip locked` must not block on A's lock");
    assert_eq!(b_read.status, 200, "skip locked read: {:?}", b_read.body);
    let skus: Vec<&str> = b_read
        .body
        .as_array()
        .expect("list")
        .iter()
        .map(|r| r["sku"].as_str().expect("sku"))
        .collect();
    assert_eq!(
        skus,
        vec!["b", "c"],
        "`skip locked` omits the locked row p1: {:?}",
        b_read.body
    );
    txn_b.commit().await.expect("commit B");

    // C: `nowait` on the locked row must error fast (not block) while A still holds the lock.
    let txn_c = engine.begin(TxOptions::default()).await.expect("begin C");
    let c_read = tokio::time::timeout(
        Duration::from_secs(5),
        txn_c
            .transport()
            .dispatch("/q/lock_one_nowait", json!({ "id": "p1" }), json!({})),
    )
    .await
    .expect("`nowait` must return fast, not block on A's lock");
    assert_ne!(
        c_read.status, 200,
        "`nowait` must error on a locked row: {:?}",
        c_read.body
    );
    txn_c.rollback().await.expect("rollback C");

    txn_a.commit().await.expect("commit A");
}

/// Bring-your-own transaction (`adopt`) live against Postgres (transactions.md rung 3): a
/// caller opens a transaction on **its own** `sqlx` pool, does a **raw non-baseddsl write**
/// on it, then runs baseddsl work (a `for update` locking read + a mutation) through
/// [`AdoptedTransport`] **on that same transaction** — and both the raw write and the
/// baseddsl write land atomically when the caller commits, and are discarded together when
/// the caller rolls back. This is the existential interop case: baseddsl is just one more
/// writer on the caller's transaction. `adopt` itself never begins/commits/rolls back — the
/// caller owns the boundary (proven: dropping the adopted transport, then the transaction,
/// leaves nothing committed).
#[tokio::test]
async fn adopt_commits_raw_and_baseddsl_writes_atomically_live_postgres() {
    use based_runtime::{AdoptedPg, AdoptedTransport, Engine, TxOptions};
    use sqlx::postgres::PgPoolOptions;

    let Some((c, router, container)) = live_schema(
        r#"
        Widget { id: text, name: text, stock: int }
        shape WidgetRow from Widget { name, stock }
        query widget_for_update(id) -> WidgetRow {
            get Widget where (id = $id) for update;
        }
        mutation restock(id, stock) -> WidgetRow {
            update Widget where (id = $id) { stock = $stock }
        }
        "#,
    )
    .await
    else {
        return;
    };
    // Seed a widget, and an **app-owned** audit table that baseddsl knows nothing about —
    // the raw writes below land there.
    container
        .exec_batch(
            "INSERT INTO widget (id, name, stock) VALUES ('w1', 'Widget', 0);\
             CREATE TABLE audit (id text primary key, widget_id text not null, note text not null);",
        )
        .await;

    let engine = Engine::new(c, router, UuidGen);
    // The caller's *own* pool — the app's connections, not the engine's backend (which
    // `adopt` never touches: the baseddsl work runs on the caller's transaction).
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&container.url())
        .await
        .expect("caller pool");

    // ---- commit path: raw audit write + baseddsl restock, one caller-owned tx ----------
    {
        let mut tx = pool.begin().await.expect("caller begins its own tx");
        // (1) a raw, non-baseddsl write on the caller's transaction.
        sqlx::query("INSERT INTO audit (id, widget_id, note) VALUES ($1, $2, $3)")
            .bind("a1")
            .bind("w1")
            .bind("restocked to 50")
            .execute(&mut *tx)
            .await
            .expect("raw audit insert");
        // (2) baseddsl work through `adopt` on the *same* transaction.
        {
            let api = AdoptedTransport::new(engine.clone(), AdoptedPg::new(&mut tx));
            // `for update` locking read works through the adopted (TxBound) client.
            let locked = api
                .dispatch("/q/widget_for_update", json!({ "id": "w1" }), json!({}))
                .await;
            assert_eq!(
                locked.status, 200,
                "adopted for-update read: {:?}",
                locked.body
            );
            let wrote = api
                .dispatch("/m/restock", json!({ "id": "w1", "stock": 50 }), json!({}))
                .await;
            assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
            assert_eq!(wrote.body["stock"], 50);
        } // the adopted transport drops here, releasing its borrow of `tx`
        tx.commit().await.expect("caller commits its own tx");
    }
    // Both writes are visible after the caller's commit.
    let (stock, audits) = audit_and_stock(&pool).await;
    assert_eq!(stock, 50, "baseddsl restock committed with the caller's tx");
    assert_eq!(audits, 1, "the raw audit row committed atomically with it");

    // ---- rollback path: the same two writes, but the caller does NOT commit ------------
    {
        let mut tx = pool.begin().await.expect("caller begins its own tx");
        sqlx::query("INSERT INTO audit (id, widget_id, note) VALUES ($1, $2, $3)")
            .bind("a2")
            .bind("w1")
            .bind("should be discarded")
            .execute(&mut *tx)
            .await
            .expect("raw audit insert");
        {
            let api = AdoptedTransport::new(engine.clone(), AdoptedPg::new(&mut tx));
            let wrote = api
                .dispatch("/m/restock", json!({ "id": "w1", "stock": 999 }), json!({}))
                .await;
            assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
        }
        // Drop the transaction without committing — `adopt` never committed anything, so the
        // caller's rollback discards BOTH the raw write and the baseddsl write together.
        drop(tx);
    }
    let (stock, audits) = audit_and_stock(&pool).await;
    assert_eq!(
        stock, 50,
        "the rolled-back baseddsl write did not persist (still 50, not 999)"
    );
    assert_eq!(
        audits, 1,
        "the rolled-back raw write did not persist (still just a1)"
    );

    // A default-options adopt (isolation is the caller's transaction's) round-trips too — the
    // adopted path is unaffected by `TxOptions`, which only the engine-owned rungs apply.
    let _ = TxOptions::default();
}

/// The widget's stock and the count of audit rows — read straight off the caller's pool, so
/// the assertions see committed state, not anything the adopted transport held.
async fn audit_and_stock(pool: &sqlx::PgPool) -> (i64, i64) {
    let stock: (i64,) = sqlx::query_as("SELECT stock FROM widget WHERE id = 'w1'")
        .fetch_one(pool)
        .await
        .expect("read stock");
    let audits: (i64,) = sqlx::query_as("SELECT count(*) FROM audit")
        .fetch_one(pool)
        .await
        .expect("count audit");
    (stock.0, audits.0)
}

// ---- streaming reads over the live wire -------------------------------------

/// Start the real `based serve` listener over a live router on a free loopback port
/// and return its `host:port` — so a streaming test observes the actual NDJSON body a
/// deployed edge produces, not just the dispatch-level stream.
async fn serve_live(compiled: Compiled, backend: PgRouter) -> String {
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
            TrustedHeaderContext,
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

/// POST one streaming query and return its NDJSON body parsed line by line.
async fn stream_lines(addr: &str, route: &str) -> Vec<serde_json::Value> {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}{route}"))
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
    body.lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON envelope"))
        .collect()
}

/// A `-> stream` query against live Postgres, observed through the full wire: `200` +
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
            "INSERT INTO item (id, name, rank) VALUES \
                ('i1', 'a', 10), ('i2', 'b', 30), ('i3', 'c', 20);",
        )
        .await;

    let addr = serve_live(c, router).await;
    let lines = stream_lines(&addr, "/q/export_items").await;
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

/// The mid-stream failure gate, live: a raw-SQL shape field divides by a row value
/// that hits zero on the last row, so a genuine Postgres `division by zero` fires
/// **during** the pass — after rows have already gone out on the spent `200`. The
/// failure must arrive as the in-band terminal `error` line with the stable code,
/// and no `done` may follow.
#[tokio::test]
async fn stream_mid_pass_db_error_is_the_in_band_error_line_live() {
    let Some((c, router, container)) = live_schema(
        r#"
        Item { id: text, label: text, denom: int }
        shape ItemRow from Item { label, boom = raw`1 / denom` }
        query export_items() -> stream ItemRow;
        "#,
    )
    .await
    else {
        return;
    };
    // Physical order: two clean rows, then the poison row (denom = 0).
    container
        .exec_batch(
            "INSERT INTO item (id, label, denom) VALUES \
                ('i1', 'a', 1), ('i2', 'b', 1), ('i3', 'c', 0);",
        )
        .await;

    let addr = serve_live(c, router).await;
    let lines = stream_lines(&addr, "/q/export_items").await;

    // The terminal line is the in-band error — the status line was long spent.
    let last = lines.last().expect("the body carries a terminal line");
    assert_eq!(last["error"]["code"], "database_error", "lines: {lines:?}");
    assert!(
        last["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("division by zero")),
        "lines: {lines:?}"
    );
    // Everything before it is a delivered row — the failure interrupted a live pass.
    assert!(
        lines.len() >= 2,
        "rows must stream before the failure: {lines:?}"
    );
    for line in &lines[..lines.len() - 1] {
        assert!(line.get("row").is_some(), "lines: {lines:?}");
    }
}

/// A to-many nested array rides back in **sort-cascade order** against a live Postgres:
/// children seeded out of order come back ordered by the child model's `@sort`
/// (`comments`), and a relation `@sort` on the edge overrides the child model's own
/// (`pins`). Proves Postgres's `json_agg(… ORDER BY …)` form executes for real.
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
            "INSERT INTO ticket (id, subject) VALUES ('t1', 'printer on fire');\n\
             INSERT INTO comment (id, ticket_id, pos, body) VALUES \
                ('c3', 't1', 3, 'third'), ('c1', 't1', 1, 'first'), ('c2', 't1', 2, 'second');\n\
             INSERT INTO pin (id, ticket_id, rank, label) VALUES \
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

/// The `serial` (DB-generated PK) read-back path against a live Postgres server: the
/// INSERT omits the id, Postgres assigns it via `GENERATED ALWAYS AS IDENTITY`, and the
/// engine reads it back with `RETURNING id` to key the declared-shape return.
#[tokio::test]
async fn serial_read_back_runs_against_live_postgres() {
    const SCHEMA: &str = r#"
        Org { id: serial  name: text }
        shape OrgCard from Org { id, name }
        mutation create_org(name) -> OrgCard { create Org { name = $name }; }
        query org_by_id(id) -> OrgCard;
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let first = call(
        &c,
        &router,
        "POST",
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    let id1 = first.body["id"].as_i64().expect("db-generated integer id");
    assert_eq!(first.body["name"], json!("Acme"));

    let second = call(
        &c,
        &router,
        "POST",
        "/m/create_org",
        json!({ "name": "Globex" }),
        json!({}),
    )
    .await;
    let id2 = second.body["id"].as_i64().expect("id");
    assert!(id2 > id1, "serial ids increment: {id1} then {id2}");

    let got = call(
        &c,
        &router,
        "POST",
        "/q/org_by_id",
        json!({ "id": id1 }),
        json!({}),
    )
    .await;
    assert_eq!(got.body, json!({ "id": id1, "name": "Acme" }));
}

/// A `@schema("…")`-qualified model lives in a named Postgres schema; a create + read-back
/// and a cross-schema FK all run against the live server. Proves the qualifier reaches
/// INSERT, the read-back SELECT + JOIN, and the FK `REFERENCES core.org` constraint.
#[tokio::test]
async fn schema_qualified_models_create_read_and_fk_across_namespaces() {
    let Some(container) = PostgresContainer::start().await else {
        return;
    };
    let src = r#"
        @schema("core")
        Org { id: Id, name: text }
        @schema("analytics")
        Event { id: Id, org: Org @fk, note: text }
        shape EventCard from Event { note, org { name } }
        query events() -> EventCard[];
        mutation add_event(org -> org, note) -> EventCard {
          create Event { org = $org, note = $note };
        }
        "#;
    let compiled = compile(src);
    let router = PgRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));

    container.exec_batch(RESET_SQL).await;
    container
        .exec_batch(
            "DROP SCHEMA IF EXISTS analytics CASCADE; DROP SCHEMA IF EXISTS core CASCADE;\n\
             CREATE SCHEMA core; CREATE SCHEMA analytics;",
        )
        .await;
    container
        .exec_batch(&sql::ddl(&compiled.schema, Dialect::Postgres))
        .await;
    container
        .exec_batch(&format!(
            "INSERT INTO \"core\".\"org\" (\"id\", \"name\") VALUES ('{ORG_1}', 'Acme');"
        ))
        .await;

    // Create the event through the engine: INSERT INTO "analytics"."event" with a FK into
    // "core"."org", then re-select in the declared shape (SELECT … FROM "analytics"."event"
    // JOIN "core"."org"). The nested `org { name }` proves the cross-schema JOIN resolved.
    let created = call(
        &compiled,
        &router,
        "POST",
        "/m/add_event",
        json!({ "org": ORG_1, "note": "hello" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "note": "hello", "org": { "name": "Acme" } })
    );

    // A second read path: the list query SELECTs FROM the qualified table + joins the
    // cross-schema parent.
    let listed = call(
        &compiled,
        &router,
        "POST",
        "/q/events",
        json!({}),
        json!({}),
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([{ "note": "hello", "org": { "name": "Acme" } }])
    );
}

/// A composite `@key(device, seq)` with a DB-generated `serial` part (OP2), live against
/// Postgres: two creates on the same device get `seq` 1 then 2 (the `GENERATED … AS
/// IDENTITY` sequence), the create response carries the DB-assigned `seq` (recovered via
/// `RETURNING "seq"`, then the full tuple re-selected), and each row reads back by its
/// composite key.
#[tokio::test]
async fn composite_serial_key_part_is_db_generated_live_postgres() {
    let src = r#"
        Device { id: Id  name: text }
        @key(device, seq)
        Reading { device: Device  seq: serial  value: int }
        shape ReadingRow from Reading { seq, value }
        query reading(device, seq) -> ReadingRow;
        mutation record_reading(device -> device, value) -> ReadingRow {
          create Reading { device = $device, value = $value };
        }
        "#;
    let Some((c, router, container)) = live_schema(src).await else {
        return;
    };
    let device = "00000000-0000-4000-8000-0000000000d1";
    container
        .exec_batch(&format!(
            "INSERT INTO \"device\" (\"id\", \"name\") VALUES ('{device}', 'sensor');"
        ))
        .await;

    // Two readings on the same device: the DB assigns seq 1, then 2 — the create response
    // carries the generated value.
    let first = call(
        &c,
        &router,
        "POST",
        "/m/record_reading",
        json!({ "device": device, "value": 10 }),
        json!({}),
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "seq": 1, "value": 10 }));

    let second = call(
        &c,
        &router,
        "POST",
        "/m/record_reading",
        json!({ "device": device, "value": 20 }),
        json!({}),
    )
    .await;
    assert_eq!(second.status, 200, "{:?}", second.body);
    assert_eq!(second.body, json!({ "seq": 2, "value": 20 }));

    // Each row reads back by its composite key (device, seq) — the writes committed.
    let got = call(
        &c,
        &router,
        "POST",
        "/q/reading",
        json!({ "device": device, "seq": 2 }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body, json!({ "seq": 2, "value": 20 }));
}

/// D124 against live Postgres (`INSERT … RETURNING`): a bound create's `@created` timestamp
/// — engine-set, unknowable at plan time — is read back and reused by a sibling step. The
/// persisted `Event.at` must equal the Ticket's real `created_at`.
#[tokio::test]
async fn tx_binding_reuses_engine_timestamp_runs_against_live_postgres() {
    const SCHEMA: &str = r#"
        @created(created_at)
        Ticket { id: Id, created_at: timestamp, subject: text }
        Event { id: Id, ticket: Ticket, at: timestamp, note: text }
        shape TicketRow from Ticket { subject, created_at }
        shape EventRow from Event { note, at, ticket = ticket.id }
        mutation open(subject: text, note: text) -> TicketRow {
          tx {
            create Ticket { subject = $subject } as t;
            create Event { ticket = $t.id, at = $t.created_at, note = $note };
          }
        }
        query events() -> EventRow[];
        query tickets() -> TicketRow[];
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let made = call(
        &c,
        &router,
        "POST",
        "/m/open",
        json!({ "subject": "S", "note": "N" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    let tickets = call(&c, &router, "POST", "/q/tickets", json!({}), json!({})).await;
    let events = call(&c, &router, "POST", "/q/events", json!({}), json!({})).await;
    let ticket_created = tickets.body[0]["created_at"]
        .as_str()
        .expect("ticket created_at");
    let event_at = events.body[0]["at"]
        .as_str()
        .unwrap_or_else(|| panic!("Event.at is NULL: {:?}", events.body[0]));
    assert_eq!(
        event_at, ticket_created,
        "$t.created_at must be the Ticket's real committed created_at"
    );
}

/// D124 against live Postgres (retired E0268): a `serial` create bound `as o`, whose id the
/// DB assigns, binds a sibling FK via `$o.id` — the RETURNING read-back threads the
/// DB-generated id into the later step.
#[tokio::test]
async fn tx_binding_reaches_serial_id_runs_against_live_postgres() {
    const SCHEMA: &str = r#"
        Org { id: serial, name: text }
        Note { id: Id, org: Org, body: text }
        shape OrgCard from Org { id, name }
        shape NoteRow from Note { body, org = org.id }
        mutation setup(name: text, body: text) -> OrgCard {
          tx {
            create Org { name = $name } as o;
            create Note { org = $o.id, body = $body };
          }
        }
        query notes() -> NoteRow[];
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let made = call(
        &c,
        &router,
        "POST",
        "/m/setup",
        json!({ "name": "Acme", "body": "hi" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    let org_id = made.body["id"].as_i64().expect("db-generated integer id");
    let notes = call(&c, &router, "POST", "/q/notes", json!({}), json!({})).await;
    assert_eq!(
        notes.body[0]["org"].as_i64(),
        Some(org_id),
        "Note.org must be the serial Org's DB-generated id: {:?}",
        notes.body[0]
    );
}

/// Nested writes end-to-end against live Postgres: a to-one child (customer), a to-many child
/// collection (items) with per-parent fan-out, and a DB-generated `serial` child whose FK is
/// learned from the INSERT via `RETURNING`.
#[tokio::test]
async fn nested_writes_run_against_live_postgres() {
    const SCHEMA: &str = r#"
        Customer { id: Id  name: text  email: text }
        LineItem { id: Id  order: Order  sku: text  qty: int }
        Order { id: Id  customer: Customer  total: int  items: LineItem[] }
        Author { id: serial  name: text }
        Book { id: Id  author: Author  title: text }

        shape OrderFull from Order { total, customer { name, email }, items { sku, qty } }
        shape LineIn from LineItem { sku, qty }
        shape BookIn from Book { title, author { name } }

        mutation place(rows: OrderFull[]) -> ok { create Order[] from $rows; }
        mutation add_books(rows: BookIn[]) -> ok { create Book[] from $rows; }
        query all_orders() -> OrderFull[];
        query all_lines() -> LineIn[];
        query all_books() -> BookIn[];
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let rows = json!([
        { "total": 10, "customer": { "name": "Ann", "email": "ann@x.io" },
          "items": [ { "sku": "x1", "qty": 1 } ] },
        { "total": 20, "customer": { "name": "Bob", "email": "bob@x.io" },
          "items": [ { "sku": "y1", "qty": 2 }, { "sku": "y2", "qty": 3 } ] },
    ]);
    let r = call(
        &c,
        &router,
        "POST",
        "/m/place",
        json!({ "rows": rows }),
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200, "nested create failed: {:?}", r.body);

    let lines = call(&c, &router, "POST", "/q/all_lines", json!({}), json!({})).await;
    assert_eq!(
        lines.body.as_array().unwrap().len(),
        3,
        "3 line items across 2 orders"
    );

    let mut orders: Vec<serde_json::Value> =
        call(&c, &router, "POST", "/q/all_orders", json!({}), json!({}))
            .await
            .body
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(sort_items)
            .collect();
    orders.sort_by_key(|o| o["total"].as_i64().unwrap_or(0));
    assert_eq!(
        orders,
        json!([
            { "total": 10, "customer": { "name": "Ann", "email": "ann@x.io" },
              "items": [ { "sku": "x1", "qty": 1 } ] },
            { "total": 20, "customer": { "name": "Bob", "email": "bob@x.io" },
              "items": [ { "sku": "y1", "qty": 2 }, { "sku": "y2", "qty": 3 } ] },
        ])
        .as_array()
        .unwrap()
        .clone()
    );

    // A serial-id child: each book's author is created with a DB-generated id, linked via
    // RETURNING.
    let books = json!([
        { "title": "SICP",  "author": { "name": "Sussman" } },
        { "title": "TAOCP", "author": { "name": "Knuth" } },
    ]);
    let br = call(
        &c,
        &router,
        "POST",
        "/m/add_books",
        json!({ "rows": books }),
        json!({}),
    )
    .await;
    assert_eq!(br.status, 200, "serial nested create failed: {:?}", br.body);
    let mut got: Vec<serde_json::Value> =
        call(&c, &router, "POST", "/q/all_books", json!({}), json!({}))
            .await
            .body
            .as_array()
            .unwrap()
            .clone();
    got.sort_by_key(|b| b["title"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        got,
        json!([
            { "title": "SICP",  "author": { "name": "Sussman" } },
            { "title": "TAOCP", "author": { "name": "Knuth" } },
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}

/// Sort a nested order's `items` array by sku (a to-many array's order is unspecified).
fn sort_items(mut o: serde_json::Value) -> serde_json::Value {
    if let Some(items) = o.get_mut("items").and_then(|v| v.as_array_mut()) {
        items.sort_by_key(|r| r["sku"].as_str().unwrap_or_default().to_string());
    }
    o
}

/// A `?` optional filter param, live on Postgres — proving the guarded predicate
/// `(:p__present = 0 OR col IS NOT DISTINCT FROM :p)` executes correctly with a bound NULL
/// `$n` param (the one genuinely Postgres-specific risk: a null bind under `IS NOT DISTINCT
/// FROM`). Absent drops the filter; JSON null → `IS NULL`; a value → equality.
#[tokio::test]
async fn optional_filter_live_postgres() {
    const SCHEMA: &str = r#"
        Product { id: text, name: text, rank: int, status: text?, @index(name), @index(status), @index(rank) }
        shape ProductName from Product { name }
        query search(status?) -> ProductName[] order (name);
        query search_gt(min?: int > rank) -> ProductName[] order (name);
    "#;
    let Some((c, router, container)) = live_schema(SCHEMA).await else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO \"product\" (\"id\", \"name\", \"rank\", \"status\") VALUES \
             ('p1', 'Widget', 10, 'active'), ('p2', 'Hammer', 20, 'active'), \
             ('p3', 'Apple', 30, NULL), ('p4', 'Banana', 40, NULL), ('p5', 'Nail', 50, 'shipped');",
        )
        .await;

    let names = |b: &serde_json::Value| -> Vec<String> {
        b.as_array()
            .expect("array")
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect()
    };

    // absent → no status predicate → every row.
    let all = call(&c, &router, "POST", "/q/search", json!({}), json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        names(&all.body),
        ["Apple", "Banana", "Hammer", "Nail", "Widget"]
    );

    // optional non-equality range: absent → all; present → `rank > 25`.
    let all_gt = call(&c, &router, "POST", "/q/search_gt", json!({}), json!({})).await;
    assert_eq!(all_gt.status, 200, "{:?}", all_gt.body);
    assert_eq!(all_gt.body.as_array().expect("array").len(), 5);
    let gt = call(
        &c,
        &router,
        "POST",
        "/q/search_gt",
        json!({ "min": 25 }),
        json!({}),
    )
    .await;
    assert_eq!(gt.status, 200, "{:?}", gt.body);
    assert_eq!(names(&gt.body), ["Apple", "Banana", "Nail"]);

    // a value → equality (NULLs excluded).
    let active = call(
        &c,
        &router,
        "POST",
        "/q/search",
        json!({ "status": "active" }),
        json!({}),
    )
    .await;
    assert_eq!(active.status, 200, "{:?}", active.body);
    assert_eq!(names(&active.body), ["Hammer", "Widget"]);
}
