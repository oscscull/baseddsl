use super::*;

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
