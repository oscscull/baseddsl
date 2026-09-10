use super::*;

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
