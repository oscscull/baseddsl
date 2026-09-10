use super::*;

/// A `?` optional filter param, live on Postgres — proving the guarded predicate
/// `(:p__present = 0 OR col IS NOT DISTINCT FROM :p)` executes correctly with a bound NULL
/// `$n` param (the one genuinely Postgres-specific risk: a null bind under `IS NOT DISTINCT
/// FROM`). Absent drops the filter; JSON null → `IS NULL`; a value → equality.
#[tokio::test]
async fn optional_filter_live_postgres() {
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
            "INSERT INTO \"product\" (\"id\", \"name\", \"rank\", \"status\") VALUES \
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
