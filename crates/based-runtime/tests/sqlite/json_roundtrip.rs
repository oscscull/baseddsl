//! End-to-end proof that a `json` column round-trips as **structured JSON** against a real
//! SQLite engine — the fix for the double-encoding bug (#16).
//!
//! A `json` column stores JSON as text, so a driver reads it back as a string. Left as-is
//! that made a `json` field arrive as a double-encoded *string* (`"{\"usd\":\"12.50\"}"`)
//! instead of the object it holds — asymmetric with writes (which accept structured JSON)
//! and unusable without a client-side `JSON.parse`. The runtime now parses `json` leaves
//! back into structured JSON at every level: top-level, a flat to-one nest, and inside a
//! to-many array. The write round-trip (create with a structured value → read it back
//! identically) is the acceptance test.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Card { id: Id, oracle_text: text }
Printing { id: Id, prices: json?, finishes: json? }
Deck { id: Id, deckcards: DeckCard[] }
DeckCard { id: Id, deck: Deck, card: Card, printing: Printing? @index deck @index card @index printing }

shape CardRow from Card { id, oracle_text }
shape PrintingRow from Printing { id, prices, finishes }
shape DeckCardRow from DeckCard { id, card -> CardRow, printing -> PrintingRow }
shape DeckRow from Deck { id, deckcards -> DeckCardRow }

query get_deck(id) -> DeckRow;
query get_deckcard(id) -> DeckCardRow;
query get_printing(id) -> PrintingRow;

mutation create_printing(prices: json?, finishes: json?) -> PrintingRow {
    create Printing { prices = $prices, finishes = $finishes };
}
"#;

// oracle_text carries a real newline (must stay escaped); prices/finishes hold structured
// JSON stored as text.
const SEED: &str = "
INSERT INTO `deck` (`id`) VALUES ('d1');
INSERT INTO `card` (`id`, `oracle_text`) VALUES ('c1', 'Line one\nLine two');
INSERT INTO `printing` (`id`, `prices`, `finishes`) VALUES ('p1', '{\"usd\":\"12.50\"}', '[\"foil\",\"etched\"]');
INSERT INTO `printing` (`id`, `prices`, `finishes`) VALUES ('p2', NULL, NULL);
INSERT INTO `deck_card` (`id`, `deck_id`, `card_id`, `printing_id`) VALUES ('dc1', 'd1', 'c1', 'p1');
";

async fn setup() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema errors: {diags:?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL: {e:?}\n{ddl}"));
    backend.execute_batch(SEED).await.expect("seed");
    (compiled, backend)
}

async fn call(
    c: &Compiled,
    b: &SqliteBackend,
    path: &str,
    args: serde_json::Value,
) -> based_runtime::WireResponse {
    let ids = SeqIdGen::default();
    dispatch(
        c,
        b,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        path,
        args,
        json!({}),
        None,
    )
    .await
}

#[tokio::test]
async fn top_level_json_is_structured() {
    let (c, b) = setup().await;
    let r = call(&c, &b, "/q/get_printing", json!({ "id": "p1" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(r.body["prices"], json!({ "usd": "12.50" }));
    assert_eq!(r.body["finishes"], json!(["foil", "etched"]));
}

#[tokio::test]
async fn null_json_stays_null() {
    let (c, b) = setup().await;
    let r = call(&c, &b, "/q/get_printing", json!({ "id": "p2" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(r.body["prices"], json!(null));
    assert_eq!(r.body["finishes"], json!(null));
}

#[tokio::test]
async fn flat_to_one_nest_json_is_structured() {
    let (c, b) = setup().await;
    let r = call(&c, &b, "/q/get_deckcard", json!({ "id": "dc1" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    // A sibling text column with a newline stays escaped; the nested json is structured.
    assert_eq!(r.body["card"]["oracle_text"], json!("Line one\nLine two"));
    assert_eq!(r.body["printing"]["prices"], json!({ "usd": "12.50" }));
    assert_eq!(r.body["printing"]["finishes"], json!(["foil", "etched"]));
}

#[tokio::test]
async fn to_many_nested_json_is_structured() {
    let (c, b) = setup().await;
    let r = call(&c, &b, "/q/get_deck", json!({ "id": "d1" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    let dc = &r.body["deckcards"][0];
    assert_eq!(dc["card"]["oracle_text"], json!("Line one\nLine two"));
    assert_eq!(dc["printing"]["prices"], json!({ "usd": "12.50" }));
    assert_eq!(dc["printing"]["finishes"], json!(["foil", "etched"]));
}

#[tokio::test]
async fn write_reads_back_identically() {
    // The round-trip north star: a structured json value written through a create reads
    // back byte-identical — no client-side transform, no double-encoding.
    let (c, b) = setup().await;
    let prices = json!({ "usd": "3.14", "eur": "2.71" });
    let finishes = json!(["nonfoil"]);
    let created = call(
        &c,
        &b,
        "/m/create_printing",
        json!({ "prices": prices, "finishes": finishes }),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(created.body["prices"], prices);
    assert_eq!(created.body["finishes"], finishes);

    // And a fresh read of the written row matches what create returned.
    let id = created.body["id"].clone();
    let read = call(&c, &b, "/q/get_printing", json!({ "id": id })).await;
    assert_eq!(read.body["prices"], prices);
    assert_eq!(read.body["finishes"], finishes);
}
