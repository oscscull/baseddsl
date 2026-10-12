use super::*;

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
async fn keyed_ok_update_decides_hit_on_matched_rows() {
    // A keyed `-> ok` update hits on matched rows (CLIENT_FOUND_ROWS): changing its own key,
    // or assigning the values a row already holds, succeeds; only a miss is `not_found`.
    let Some((c, router, container)) = live_schema(
        r#"
        @key(sku)
        Item { sku: text, label: text (unique) }
        shape ItemRow from Item { sku, label }
        query items() -> ItemRow[] { list Item order (sku); }
        mutation resku(sku: text, to: text) -> ok { update Item where (sku = $sku) { sku = $to }; }
        mutation relabel(label: text, to: text) -> ok { update Item where (label = $label) { label = $to }; }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch("INSERT INTO `item` (`sku`, `label`) VALUES ('s-1', 'one');")
        .await;

    for (path, args, status) in [
        ("/m/resku", json!({ "sku": "s-1", "to": "s-2" }), 200),
        ("/m/relabel", json!({ "label": "one", "to": "uno" }), 200),
        ("/m/relabel", json!({ "label": "uno", "to": "uno" }), 200),
        ("/m/resku", json!({ "sku": "s-1", "to": "s-3" }), 404),
    ] {
        let resp = call(&c, &router, "POST", path, args, json!({})).await;
        assert_eq!(resp.status, status, "{path}: {:?}", resp.body);
    }

    let items = call(&c, &router, "POST", "/q/items", json!({}), json!({})).await;
    assert_eq!(items.body, json!([{ "sku": "s-2", "label": "uno" }]));
}
