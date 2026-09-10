use super::*;

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
