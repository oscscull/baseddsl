use super::*;

const SCHEMA: &str = r#"
    AnyCard { id: text, name: text, printings: AnyPrinting[], @index(name) }
    AnyPrinting { id: text, card: AnyCard, digital: bool, @index(card) }
    AnyCategory { id: text, name: text, parent: AnyCategory?, children: AnyCategory[] (AnyCategory.parent), @index(name) }
    shape CardName from AnyCard { name }
    shape CategoryName from AnyCategory { name }
    query paper() -> CardName[] { list AnyCard where (printings.any(digital = false)) order (name); }
    query unprinted() -> CardName[] { list AnyCard where (not printings.any()) order (name); }
    query categories() -> CategoryName[] order (name);
    mutation prune() -> ok { delete AnyCategory where (not children.any()); }
"#;

fn names(b: &serde_json::Value) -> Vec<String> {
    b.as_array()
        .expect("array")
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

/// `rel.any(…)` live on MariaDB: an existence filter over a to-many relation, and a
/// self-referential `.any` in a DELETE (read through a materialized derived table).
#[tokio::test]
async fn relation_any_live_mariadb() {
    let Some((c, router, container)) = live_schema(SCHEMA).await else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO `any_card` (`id`, `name`) VALUES ('c1', 'Bolt'), ('c2', 'Arena Only'), ('c3', 'Blank');\n\
             INSERT INTO `any_printing` (`id`, `card_id`, `digital`) VALUES \
             ('p1', 'c1', FALSE), ('p2', 'c1', FALSE), ('p3', 'c2', TRUE);\n\
             INSERT INTO `any_category` (`id`, `name`, `parent_id`) VALUES \
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
