use super::*;

const SCHEMA: &str = r#"
    LikeCard { id: text, name: text, body: text?, @index(name) }
    shape LikeName from LikeCard { name }
    query all_of(terms: text[]) -> LikeName[] { list LikeCard where (body ~ all $terms) order (name); }
    query any_of(terms: text[]) -> LikeName[] { list LikeCard where (body ~ any $terms) order (name); }
    query none_of(terms: text[]) -> LikeName[] { list LikeCard where (not (body ~ any $terms)) order (name); }
    query all_of_opt(terms?: text[]) -> LikeName[] { list LikeCard where (body ~ all $terms) order (name); }
"#;

/// `~ all` / `~ any` live on Postgres: many, one, and empty pattern lists, the `not` exclusion
/// over a NULL column, and an absent optional list. `Nil`'s body is the text `null`, which
/// an empty list's placeholder element must never match.
#[tokio::test]
async fn like_lists_live_postgres() {
    let Some((c, router, container)) = live_schema(SCHEMA).await else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO \"like_card\" (\"id\", \"name\", \"body\") VALUES \
             ('c1', 'Bolt', 'deal 3 damage to any target'), ('c2', 'Counter', 'counter target spell'), \
             ('c3', 'Growth', 'target creature gets +3/+3'), ('c4', 'Vanilla', NULL), ('c5', 'Nil', 'null');",
        )
        .await;
    let names = |path: &'static str, args: serde_json::Value| {
        let (c, router) = (&c, &router);
        async move {
            let r = call(c, router, "POST", path, args, json!({})).await;
            assert_eq!(r.status, 200, "{path}: {:?}", r.body);
            r.body
                .as_array()
                .expect("array")
                .iter()
                .map(|row| row["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    let every = ["Bolt", "Counter", "Growth", "Nil", "Vanilla"];

    let terms = json!({ "terms": ["%target%", "%damage%"] });
    assert_eq!(names("/q/all_of", terms).await, ["Bolt"]);
    let terms = json!({ "terms": ["%target%"] });
    assert_eq!(
        names("/q/all_of", terms).await,
        ["Bolt", "Counter", "Growth"]
    );
    assert_eq!(names("/q/all_of", json!({ "terms": [] })).await, every);

    let terms = json!({ "terms": ["%counter%", "%creature%"] });
    assert_eq!(names("/q/any_of", terms).await, ["Counter", "Growth"]);
    assert!(names("/q/any_of", json!({ "terms": [] })).await.is_empty());

    let terms = json!({ "terms": ["%damage%"] });
    assert_eq!(
        names("/q/none_of", terms).await,
        ["Counter", "Growth", "Nil", "Vanilla"]
    );
    assert_eq!(names("/q/none_of", json!({ "terms": [] })).await, every);

    assert_eq!(names("/q/all_of_opt", json!({})).await, every);
}
