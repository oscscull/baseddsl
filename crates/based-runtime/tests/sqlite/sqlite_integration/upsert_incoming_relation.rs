//! A bulk upsert re-points a relation from the incoming row (`token = incoming.token`),
//! for single-column and composite-key relations.

use super::*;

#[tokio::test]
async fn bulk_upsert_moves_a_relation_from_incoming() {
    let c = compile_sqlite(
        r#"
        Token { id: Id, name: text }
        TokenPrinting { id: serial, scryfall_id: text (unique), token: Token, data: json }
        shape TokenPrintingIn from TokenPrinting { scryfall_id token { id } data }
        shape PrintingRow from TokenPrinting { scryfall_id, token = token.name, data }
        query printings() -> PrintingRow[] { list TokenPrinting order (scryfall_id); }
        mutation import(rows: TokenPrintingIn[]) -> ok {
          create TokenPrinting[] from $rows
            on conflict (scryfall_id) update { token = incoming.token, data = incoming.data };
        }
        "#,
    );
    let b = SqliteBackend::in_memory().expect("open sqlite");
    b.execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("ddl");
    b.execute_batch("INSERT INTO `token` (`id`, `name`) VALUES ('t-1', 'Goblin'), ('t-2', 'Elf');")
        .await
        .expect("seed");

    let rows = |token: &str, v: i64| {
        json!({ "rows": [
            { "scryfall_id": "s-1", "token": { "id": token }, "data": { "v": v } },
        ] })
    };
    let first = call(&c, &b, "POST", "/m/import", rows("t-1", 1), json!({})).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    let moved = call(&c, &b, "POST", "/m/import", rows("t-2", 2), json!({})).await;
    assert_eq!(moved.status, 200, "{:?}", moved.body);

    let got = call(&c, &b, "POST", "/q/printings", json!({}), json!({})).await;
    assert_eq!(
        got.body,
        json!([{ "scryfall_id": "s-1", "token": "Elf", "data": { "v": 2 } }])
    );
}

#[tokio::test]
async fn bulk_upsert_moves_a_composite_relation_from_incoming() {
    let c = compile_sqlite(
        r#"
        @key(code, term)
        Course { code: text, term: text, title: text }
        Seat { id: Id, tag: text (unique), course: Course }
        shape SeatIn from Seat { tag course { code term } }
        shape SeatRow from Seat { tag, title = course.title }
        query seats() -> SeatRow[] { list Seat order (tag); }
        mutation import(rows: SeatIn[]) -> ok {
          create Seat[] from $rows on conflict (tag) update { course = incoming.course };
        }
        "#,
    );
    let b = SqliteBackend::in_memory().expect("open sqlite");
    b.execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("ddl");
    b.execute_batch(
        "INSERT INTO `course` (`code`, `term`, `title`) VALUES ('CS1', 'F', 'Fall'), ('CS1', 'S', 'Spring');",
    )
    .await
    .expect("seed");

    let rows =
        |term: &str| json!({ "rows": [{ "tag": "a", "course": { "code": "CS1", "term": term } }] });
    let first = call(&c, &b, "POST", "/m/import", rows("F"), json!({})).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    let moved = call(&c, &b, "POST", "/m/import", rows("S"), json!({})).await;
    assert_eq!(moved.status, 200, "{:?}", moved.body);

    let got = call(&c, &b, "POST", "/q/seats", json!({}), json!({})).await;
    assert_eq!(got.body, json!([{ "tag": "a", "title": "Spring" }]));
}
