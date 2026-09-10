use super::*;

/// Keyset-cursor pagination, proven against a real engine: paging a `page (2)`
/// keyset query walks the whole set exactly once — each page returns the next window and
/// an opaque cursor, the final short page returns a `null` cursor, and the cursor works
/// even though the sort basis (`rank`, `id`) is not projected (the runtime strips the
/// hidden `__keyset_*` columns from the response). A tampered cursor is a 400.
#[tokio::test]
async fn keyset_pagination_walks_the_set_end_to_end() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Item { id: Id, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query items() -> ItemCard[] { list Item order (rank asc) page (2); }
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
            INSERT INTO `item` (`id`, `name`, `rank`) VALUES
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30),
                ('i4', 'd', 40), ('i5', 'e', 50);
            "#,
        )
        .await
        .expect("seed");

    let page = |args: serde_json::Value| call(&c, &backend, "POST", "/q/items", args, json!({}));

    // Page 1 (no cursor): the two lowest-ranked rows + a "more" cursor (a full page).
    // Rows carry only the projected `{name, rank}` — the hidden sort-key columns are
    // stripped even though they drive the cursor.
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

/// `with count`, proven against a real engine: the page envelope carries the live-row
/// `total` beside the bounded window; a page without `with count` omits the field.
#[tokio::test]
async fn with_count_page_carries_total_end_to_end() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Item { id: Id, name: text, rank: int }
        shape ItemCard from Item { name }
        query counted() -> ItemCard[] { list Item order (rank asc) page (2) offset with count; }
        query windowed() -> ItemCard[] { list Item order (rank asc) page (2); }
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
            INSERT INTO `item` (`id`, `name`, `rank`) VALUES
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30),
                ('i4', 'd', 40), ('i5', 'e', 50);
            "#,
        )
        .await
        .expect("seed");

    // The counted page: two rows in the window, the whole live set in `total`.
    let p = call(&c, &backend, "POST", "/q/counted", json!({}), json!({})).await;
    assert_eq!(p.status, 200, "{:?}", p.body);
    assert_eq!(p.body["rows"], json!([{ "name": "a" }, { "name": "b" }]));
    assert_eq!(p.body["total"], json!(5));

    // Without `with count` the envelope has no `total` key at all.
    let w = call(&c, &backend, "POST", "/q/windowed", json!({}), json!({})).await;
    assert_eq!(w.status, 200, "{:?}", w.body);
    assert!(
        !w.body.as_object().unwrap().contains_key("total"),
        "{:?}",
        w.body
    );
}

/// `list distinct … page (…) offset with count`: the `total` must be the number of
/// **deduped** rows, not the raw pre-dedup row count. Five products across two categories
/// under a `distinct` projection of `category` yield two result rows, so `total` is 2 —
/// a `COUNT(*)` over the undeduped rows would wrongly report 5 and break page-count math.
#[tokio::test]
async fn distinct_with_count_totals_deduped_rows() {
    let c = compile_sqlite(
        r#"
        Product { id: Id, name: text, category: text }
        shape CategoryName from Product { category }
        query distinct_paged() -> CategoryName[] {
          list distinct Product order (category) page (10) offset with count;
        }
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
            INSERT INTO `product` (`id`, `name`, `category`) VALUES
                ('p1', 'Widget', 'tools'), ('p2', 'Hammer', 'tools'),
                ('p3', 'Apple', 'food'),   ('p4', 'Banana', 'food'),
                ('p5', 'Nail', 'tools');
            "#,
        )
        .await
        .expect("seed");

    let p = call(
        &c,
        &backend,
        "POST",
        "/q/distinct_paged",
        json!({}),
        json!({}),
    )
    .await;
    assert_eq!(p.status, 200, "{:?}", p.body);
    assert_eq!(
        p.body["rows"],
        json!([{ "category": "food" }, { "category": "tools" }])
    );
    // The result set has two distinct rows — the count must match, not the five raw rows.
    assert_eq!(
        p.body["total"],
        json!(2),
        "distinct `with count` overcounted"
    );
}
