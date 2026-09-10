use super::*;

/// A to-**many** nested shape array (`items { … }`) returns a JSON array of
/// sub-objects end-to-end: codegen aggregates the child rows into an `items[]` JSON-array
/// column (correlated subquery + SQLite `json_group_array`), the live SELECT returns it as
/// a string, and the runtime parses it into a real JSON array — proven against a real
/// engine. Also asserts a parent with no children returns `[]`, and the child's soft-delete
/// tombstone is respected (a deleted item is excluded from the array).
#[tokio::test]
async fn nested_to_many_query_returns_json_array() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Order { id: Id, total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { id: Id, order: Order, sku: text, qty: int, deleted_at: timestamp? }
        shape OrderCard from Order { total, items { sku, qty } }
        query order_by_id(id) -> OrderCard;
        query orders() -> OrderCard[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `order` (`id`, `total`) VALUES ('o1', 500), ('o2', 0);
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`) VALUES
                ('i1', 'o1', 'ABC', 2), ('i2', 'o1', 'XYZ', 5);
            -- a soft-deleted item on o1 must be excluded from the array.
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`, `deleted_at`)
                VALUES ('i3', 'o1', 'GONE', 9, '2020-01-01 00:00:00');
            "#,
        )
        .await
        .expect("seed");

    // `get`: the child rows ride back nested under `items`, not as a flat string column.
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
        json!({
            "total": 500,
            "items": [{ "sku": "ABC", "qty": 2 }, { "sku": "XYZ", "qty": 5 }]
        }),
        "soft-deleted item excluded, remaining children nested"
    );

    // `list`: o2 has no items → an empty array (not null, not a missing field).
    let listed = call(&c, &backend, "POST", "/q/orders", json!({}), json!({})).await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([
            { "total": 500, "items": [{ "sku": "ABC", "qty": 2 }, { "sku": "XYZ", "qty": 5 }] },
            { "total": 0, "items": [] }
        ])
    );
}

/// The flagship **self-referential** to-many (`User.invited_users`): a User joined to
/// itself under a distinct subquery alias. Proven end-to-end against a real engine — the
/// correlated subquery's `s<n>_user` alias never collides with the outer `user` row, so a
/// user's invitees nest correctly.
#[tokio::test]
async fn nested_self_referential_to_many_returns_json_array() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        User {
          id: Id
          name: text
          invited_by: User?
          invited_users: User[] (User.invited_by)
        }
        shape UserCard from User { name, invited_users { name } }
        query user_by_id(id) -> UserCard;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `invited_by_id`) VALUES
                ('u1', 'Ada', NULL), ('u2', 'Bob', 'u1'), ('u3', 'Cy', 'u1');
            "#,
        )
        .await
        .expect("seed");

    // Ada (u1) invited Bob + Cy: both nest under `invited_users`.
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
        json!({ "name": "Ada", "invited_users": [{ "name": "Bob" }, { "name": "Cy" }] })
    );

    // Bob invited no one → an empty array.
    let leaf = call(
        &c,
        &backend,
        "POST",
        "/q/user_by_id",
        json!({ "id": "u2" }),
        json!({}),
    )
    .await;
    assert_eq!(leaf.body, json!({ "name": "Bob", "invited_users": [] }));
}

/// A to-many nested array rides back in **sort-cascade order**, proven live: children
/// seeded out of order come back ordered by the child model's `@sort` (`comments`), and
/// a relation `@sort` on the edge overrides the child model's own (`pins`). The ORDER BY
/// lives inside the JSON aggregate, so the outer query's shape is untouched.
#[tokio::test]
async fn nested_to_many_rows_ride_in_sort_cascade_order() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Ticket {
          id: Id
          subject: text
          comments: Comment[]
          pins: Pin[] @sort(rank desc)
        }
        @sort(pos asc)
        Comment { id: Id, ticket: Ticket, pos: int, body: text }
        @sort(rank asc)
        Pin { id: Id, ticket: Ticket, rank: int, label: text }
        shape TicketDetail from Ticket { subject, comments { body }, pins { label } }
        query ticket_by_id(id) -> TicketDetail;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `ticket` (`id`, `subject`) VALUES ('t1', 'printer on fire');
            -- seeded out of `pos` order: the array order must come from the sort, not insertion.
            INSERT INTO `comment` (`id`, `ticket_id`, `pos`, `body`) VALUES
                ('c3', 't1', 3, 'third'), ('c1', 't1', 1, 'first'), ('c2', 't1', 2, 'second');
            INSERT INTO `pin` (`id`, `ticket_id`, `rank`, `label`) VALUES
                ('p1', 't1', 1, 'low'), ('p3', 't1', 3, 'top'), ('p2', 't1', 2, 'mid');
            "#,
        )
        .await
        .expect("seed");

    let resp = call(
        &c,
        &backend,
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

/// A **named-shape** nest (`placed_by -> UserRef`, `items -> ItemRow`) returns the same
/// nested JSON an inline nest does, end-to-end against a real engine: the reference is a
/// pure body expansion (same SQL, same `nest_row`/array reassembly), the payoff being the
/// shared nominal type on the client. Covers to-one and to-many refs in one shape, with a
/// soft-deleted child excluded exactly as the nest context dictates.
#[tokio::test]
async fn named_shape_nest_returns_nested_json() {
    let c = compile_sqlite(
        r#"
        User { id: Id, name: text, email: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int, items: OrderItem[] }
        @sort(id asc)
        @soft_delete(deleted_at)
        OrderItem { id: Id, order: Order, sku: text, qty: int, deleted_at: timestamp? }
        shape UserRef from User { name, email }
        shape ItemRow from OrderItem { sku, qty }
        shape OrderDetail from Order {
          total
          placed_by -> UserRef
          items -> ItemRow
        }
        query order_detail(id) -> OrderDetail;
        query order_details() -> OrderDetail[];
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            r#"
            INSERT INTO `user` (`id`, `name`, `email`) VALUES ('u1', 'Ada', 'a@x.com');
            INSERT INTO `order` (`id`, `placed_by_id`, `total`) VALUES
                ('o1', 'u1', 500), ('o2', 'u1', 0);
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`) VALUES
                ('i1', 'o1', 'ABC', 2);
            -- a soft-deleted item must be excluded, exactly as with an inline nest.
            INSERT INTO `order_item` (`id`, `order_id`, `sku`, `qty`, `deleted_at`)
                VALUES ('i2', 'o1', 'GONE', 9, '2020-01-01 00:00:00');
            "#,
        )
        .await
        .expect("seed");

    // `get`: buyer nests as the named `UserRef` projection, items as `ItemRow` elements.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_detail",
        json!({ "id": "o1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "total": 500,
            "placed_by": { "name": "Ada", "email": "a@x.com" },
            "items": [{ "sku": "ABC", "qty": 2 }]
        })
    );

    // `list`: every row reassembles; a childless order yields `[]`.
    let listed = call(
        &c,
        &backend,
        "POST",
        "/q/order_details",
        json!({}),
        json!({}),
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([
            {
                "total": 500,
                "placed_by": { "name": "Ada", "email": "a@x.com" },
                "items": [{ "sku": "ABC", "qty": 2 }]
            },
            {
                "total": 0,
                "placed_by": { "name": "Ada", "email": "a@x.com" },
                "items": []
            }
        ])
    );
}

/// The commerce example's `order_detail` (its `OrderDetail` nests `placed_by ->
/// UserRef`) runs live: the worked example's named-shape reference is executable,
/// not just documentation.
#[tokio::test]
async fn commerce_order_detail_nests_named_user_ref() {
    let c = commerce();
    let backend = seeded_backend(&c).await;
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/order_detail",
        json!({ "id": "order-1" }),
        json!({ "org": "org-1" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "status": "paid",
            "total": "500.00",
            "placed_by": { "name": "Ada", "email": "a@x.com" }
        })
    );
}
