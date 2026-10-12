use super::*;

const SCHEMA: &str = r#"
    Card { id: text, name: text, printings: Printing[], @index(name) }
    Printing { id: text, card: Card, digital: bool, @index(card) }
    Category { id: text, name: text, parent: Category?, children: Category[] (Category.parent), @index(name) }
    shape CardName from Card { name }
    shape CategoryName from Category { name }
    query paper() -> CardName[] { list Card where (printings.any(digital = false)) order (name); }
    query unprinted() -> CardName[] { list Card where (not printings.any()) order (name); }
    query categories() -> CategoryName[] order (name);
    mutation prune() -> ok { delete Category where (not children.any()); }
"#;

fn names(b: &serde_json::Value) -> Vec<String> {
    b.as_array()
        .expect("array")
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

/// `rel.any(…)` live on Postgres: an existence filter over a to-many relation, and a
/// self-referential `.any` in a DELETE (a correlated EXISTS).
#[tokio::test]
async fn relation_any_live_postgres() {
    let Some((c, router, container)) = live_schema(SCHEMA).await else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO card (id, name) VALUES ('c1', 'Bolt'), ('c2', 'Arena Only'), ('c3', 'Blank');\n\
             INSERT INTO printing (id, card_id, digital) VALUES \
             ('p1', 'c1', FALSE), ('p2', 'c1', FALSE), ('p3', 'c2', TRUE);\n\
             INSERT INTO category (id, name, parent_id) VALUES \
             ('k1', 'Root', NULL), ('k2', 'Mid', 'k1'), ('k3', 'Leaf', 'k2');",
        )
        .await;

    let paper = call(&c, &router, "POST", "/q/paper", json!({}), json!({})).await;
    assert_eq!(paper.status, 200, "{:?}", paper.body);
    // Two paper printings of Bolt still yield one row.
    assert_eq!(names(&paper.body), ["Bolt"]);

    let none = call(&c, &router, "POST", "/q/unprinted", json!({}), json!({})).await;
    assert_eq!(names(&none.body), ["Blank"]);

    let pruned = call(&c, &router, "POST", "/m/prune", json!({}), json!({})).await;
    assert_eq!(pruned.status, 200, "{:?}", pruned.body);
    let left = call(&c, &router, "POST", "/q/categories", json!({}), json!({})).await;
    assert_eq!(names(&left.body), ["Mid", "Root"]);
}
