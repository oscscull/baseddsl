use super::*;

/// Nested writes end-to-end against live MariaDB: a to-one child (customer), a to-many child
/// collection (items) with per-parent fan-out, and a DB-generated `serial` child whose FK is
/// learned from the INSERT via the `LAST_INSERT_ID()` range (MariaDB has no `RETURNING`).
#[tokio::test]
async fn nested_writes_run_against_live_mariadb() {
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

    // A serial-id child: each book's author gets a DB-generated id, linked via the
    // LAST_INSERT_ID() range.
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

/// A `?` optional filter param, live on MariaDB — proving the guarded predicate
/// `(:p__present = 0 OR col <=> :p)` executes correctly with a bound NULL param under the
/// null-safe `<=>` operator. Absent drops the filter; JSON null → `IS NULL`; a value → equality.
#[tokio::test]
async fn optional_filter_live_mariadb() {
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
            "INSERT INTO `product` (`id`, `name`, `rank`, `status`) VALUES \
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
