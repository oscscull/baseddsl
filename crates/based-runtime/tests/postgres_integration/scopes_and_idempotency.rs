use super::*;

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
