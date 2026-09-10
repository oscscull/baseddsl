use super::*;

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
