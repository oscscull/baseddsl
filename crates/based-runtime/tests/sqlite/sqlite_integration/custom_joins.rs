use super::*;

#[tokio::test]
async fn custom_join_condition_resolves_legacy_keys_end_to_end() {
    // A relation with a custom `on:` join (legacy keys, not `<field>_id`): `buyer` links
    // `order.user_ref` to `user.legacy_id`, neither of which is a conventional FK column.
    // Proven live: codegen renders the declared condition as the join `ON` (forward) and
    // the to-many correlation (inverse), so a real SQLite row resolves through both — and
    // the custom-join edge emits no phantom `buyer_id` column (the create seeds without it).
    let c = compile_sqlite(
        r#"
        User { id: Id, name: text, legacy_id: int, orders: Order[] (Order.buyer) }
        Order { id: Id, note: text, user_ref: int, buyer: User (on: order.user_ref = user.legacy_id) }
        shape OrderCard from Order { note, buyer { name } }
        query order_by_id(id) -> OrderCard;
        shape UserOrders from User { name, orders { note } }
        query user_by_id(id) -> UserOrders;
        "#,
    );
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    // The custom-join edge owns no conventional FK column.
    assert!(!ddl.contains("buyer_id"), "no phantom FK column:\n{ddl}");

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    // Ada's surrogate id is `u1` but her legacy id is 42; the order links by legacy id.
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `legacy_id`) VALUES ('u1', 'Ada', 42);
            INSERT INTO `order` (`id`, `note`, `user_ref`) VALUES ('o1', 'first', 42), ('o2', 'second', 42);
            "#,
        )
        .await
        .expect("seed");

    // Forward: the join follows `user_ref = legacy_id`, not a nonexistent `buyer_id = id`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "note": "first", "buyer": { "name": "Ada" } })
    );

    // Inverse to-many: the correlation aggregates both orders sharing Ada's legacy id.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/user_by_id",
        json!({ "id": "u1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "name": "Ada", "orders": [ { "note": "first" }, { "note": "second" } ] })
    );
}
