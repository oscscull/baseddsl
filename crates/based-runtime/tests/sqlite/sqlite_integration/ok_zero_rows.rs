//! `-> ok` writes matching zero rows: a filtered bulk write is a no-op success, a keyed
//! single-row write is `not_found`. A keyed update that changes its own key still hits.

use super::*;

const SCHEMA: &str = r#"
    Card { id: Id, name: text (unique), power: int }
    @soft_delete(deleted_at)
    Token { id: Id, deleted_at: timestamp?, label: text }
    shape CardRow from Card { name, power }
    query cards() -> CardRow[] { list Card order (name); }
    mutation reset_power() -> ok { update Card where (power != 0) { power = 0 }; }
    mutation purge_weak() -> ok { delete Card where (power < 1); }
    mutation drop_tokens(label: text) -> ok { delete Token where (label = $label); }
    mutation set_power(id: Id, power: int) -> ok { update Card where (id = $id) { power = $power }; }
    mutation power_by_name(name: text, power: int) -> ok { update Card where (name = $name) { power = $power }; }
    mutation remove(id: Id) -> ok { delete Card where (id = $id); }
    mutation rename(name: text, to: text) -> ok { update Card where (name = $name) { name = $to }; }
    @key(sku)
    Item { sku: text, label: text }
    shape ItemRow from Item { sku, label }
    query items() -> ItemRow[] { list Item order (sku); }
    mutation resku(sku: text, to: text) -> ok { update Item where (sku = $sku) { sku = $to }; }
"#;

async fn backend(c: &Compiled) -> SqliteBackend {
    let b = SqliteBackend::in_memory().expect("open sqlite");
    b.execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("ddl");
    b.execute_batch(
        "INSERT INTO `card` (`id`, `name`, `power`) VALUES ('c-1', 'A', 0), ('c-2', 'B', 3);",
    )
    .await
    .expect("seed");
    b.execute_batch("INSERT INTO `item` (`sku`, `label`) VALUES ('s-1', 'one');")
        .await
        .expect("seed items");
    b
}

#[tokio::test]
async fn filtered_bulk_update_matching_nothing_succeeds() {
    let c = compile_sqlite(SCHEMA);
    let b = backend(&c).await;
    let first = call(&c, &b, "POST", "/m/reset_power", json!({}), json!({})).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    // Every power is now zero, so the same filter matches no row.
    let again = call(&c, &b, "POST", "/m/reset_power", json!({}), json!({})).await;
    assert_eq!(again.status, 200, "{:?}", again.body);
    let cards = call(&c, &b, "POST", "/q/cards", json!({}), json!({})).await;
    assert_eq!(
        cards.body,
        json!([{ "name": "A", "power": 0 }, { "name": "B", "power": 0 }])
    );
}

#[tokio::test]
async fn filtered_bulk_delete_matching_nothing_succeeds() {
    let c = compile_sqlite(SCHEMA);
    let b = backend(&c).await;
    let first = call(&c, &b, "POST", "/m/purge_weak", json!({}), json!({})).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    let again = call(&c, &b, "POST", "/m/purge_weak", json!({}), json!({})).await;
    assert_eq!(again.status, 200, "{:?}", again.body);
    let soft = call(
        &c,
        &b,
        "POST",
        "/m/drop_tokens",
        json!({ "label": "x" }),
        json!({}),
    )
    .await;
    assert_eq!(soft.status, 200, "{:?}", soft.body);
    let cards = call(&c, &b, "POST", "/q/cards", json!({}), json!({})).await;
    assert_eq!(cards.body, json!([{ "name": "B", "power": 3 }]));
}

#[tokio::test]
async fn keyed_write_matching_nothing_is_not_found() {
    let c = compile_sqlite(SCHEMA);
    let b = backend(&c).await;
    for (route, input) in [
        ("/m/set_power", json!({ "id": "missing", "power": 1 })),
        ("/m/power_by_name", json!({ "name": "missing", "power": 1 })),
        ("/m/remove", json!({ "id": "missing" })),
    ] {
        let resp = call(&c, &b, "POST", route, input, json!({})).await;
        assert_eq!(resp.status, 404, "{route}: {:?}", resp.body);
        assert_eq!(resp.body["error"]["code"], "not_found");
    }
}

#[tokio::test]
async fn keyed_update_changing_its_unique_key_succeeds() {
    let c = compile_sqlite(SCHEMA);
    let b = backend(&c).await;
    let input = json!({ "name": "A", "to": "Z" });
    let resp = call(&c, &b, "POST", "/m/rename", input, json!({})).await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    let cards = call(&c, &b, "POST", "/q/cards", json!({}), json!({})).await;
    assert_eq!(
        cards.body,
        json!([{ "name": "B", "power": 3 }, { "name": "Z", "power": 0 }])
    );
    // Same-value assignment still matches the row.
    let input = json!({ "name": "B", "to": "B" });
    let same = call(&c, &b, "POST", "/m/rename", input, json!({})).await;
    assert_eq!(same.status, 200, "{:?}", same.body);
    let input = json!({ "name": "A", "to": "Y" });
    let miss = call(&c, &b, "POST", "/m/rename", input, json!({})).await;
    assert_eq!(miss.status, 404, "{:?}", miss.body);
}

#[tokio::test]
async fn keyed_update_changing_its_primary_key_succeeds() {
    let c = compile_sqlite(SCHEMA);
    let b = backend(&c).await;
    let input = json!({ "sku": "s-1", "to": "s-2" });
    let resp = call(&c, &b, "POST", "/m/resku", input, json!({})).await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    let items = call(&c, &b, "POST", "/q/items", json!({}), json!({})).await;
    assert_eq!(items.body, json!([{ "sku": "s-2", "label": "one" }]));
    let input = json!({ "sku": "s-1", "to": "s-3" });
    let miss = call(&c, &b, "POST", "/m/resku", input, json!({})).await;
    assert_eq!(miss.status, 404, "{:?}", miss.body);
    assert_eq!(miss.body["error"]["code"], "not_found");
}
