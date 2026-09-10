use super::*;

/// Keyset pagination over a **nullable** sort column, proven against a real engine: a
/// plain `col < :cursor` predicate would drop every NULL-valued row (SQL `NULL < v` is
/// `NULL`, not true), silently losing rows from the walk. The NULL-aware predicate keeps
/// the walk correct — every row appears exactly once, NULL-scored rows included, in the
/// dialect's NULL sort position (SQLite: NULLs last under `desc`).
#[tokio::test]
async fn keyset_over_nullable_sort_column_loses_no_rows() {
    let c = compile_sqlite(
        r#"
        @sort(id asc)
        Item { id: Id, name: text, score: int? }
        shape ItemCard from Item { name, score }
        query items() -> ItemCard[] { list Item order (score desc) page (2); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend.execute_batch(&ddl).await.unwrap();
    backend
        .execute_batch(
            r#"INSERT INTO `item` (`id`,`name`,`score`) VALUES
               ('i1','a',30),('i2','b',20),('i3','c',NULL),('i4','d',10),('i5','e',NULL);"#,
        )
        .await
        .unwrap();
    let page = |args: serde_json::Value| call(&c, &backend, "POST", "/q/items", args, json!({}));

    // Walk every page, collecting the ordered names and guarding against a runaway loop.
    let mut ordered: Vec<String> = Vec::new();
    let mut cursor = json!({});
    for _ in 0..10 {
        let p = page(cursor.clone()).await;
        assert_eq!(p.status, 200, "{:?}", p.body);
        for row in p.body["rows"].as_array().unwrap() {
            ordered.push(row["name"].as_str().unwrap().to_string());
        }
        match p.body["cursor"].as_str() {
            Some(cur) => cursor = json!({ "cursor": cur }),
            None => break,
        }
    }
    // `score desc` then `id asc`; SQLite ranks NULL lowest, so the two NULL rows trail,
    // ordered among themselves by the id tiebreaker (i3 before i5).
    assert_eq!(
        ordered,
        vec!["a", "b", "d", "c", "e"],
        "nullable-sort keyset walk: complete, correctly ordered, no dropped/repeated rows"
    );
}
