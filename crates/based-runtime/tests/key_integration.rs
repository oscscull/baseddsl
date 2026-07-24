//! End-to-end proof of the `@key(field)` natural single-column primary key against a
//! **real** SQLite engine — no mock.
//!
//! A `@key` model has no surrogate `id`: a declared column *is* the primary key, supplied
//! by the caller on create and read back by that column (not by a generated id). A relation
//! into a `@key` model carries the key's own type and references the nominated column. This
//! test runs the verbatim `based gen sql` DDL + the runtime's lowered DML against a live
//! in-memory SQLite database, so a passing run means the whole natural-key path — key-less
//! `id`, natural-key read-back, FK-to-natural-key, get-by-key, and a to-many nest correlated
//! on the natural key — actually executes.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
@key(sku)
Product { sku: text, name: text, reviews: Review[] }

Review { id: Id, product: Product, body: text, @index product }

shape ProductCard from Product { sku, name }
shape ProductWithReviews from Product { sku, name, reviews { body } }
shape ReviewCard from Review { id, body, product = product.sku }

mutation create_product(sku, name) -> ProductCard {
  create Product { sku = $sku, name = $name };
}
mutation create_review(product -> product, body) -> ReviewCard {
  create Review { product = $product, body = $body };
}
query product_by_sku(sku) -> ProductCard;
query product_reviews(sku) -> ProductWithReviews;
"#;

async fn key_backend() -> (Compiled, SqliteBackend) {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean: {diags:?}"
    );
    let ddl = sql::ddl(&schema, Dialect::Sqlite);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("key DDL failed to execute: {e:?}\n{ddl}"));
    (compiled, backend)
}

async fn call(
    compiled: &Compiled,
    backend: &SqliteBackend,
    path: &str,
    args: serde_json::Value,
) -> based_runtime::WireResponse {
    let ids = SeqIdGen::default();
    dispatch(
        compiled,
        backend,
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
async fn create_reads_back_by_the_natural_key() {
    // The create supplies the natural key; the row reads back keyed on that column (no
    // generated id). The key crosses the wire as its own type — a JSON string.
    let (c, backend) = key_backend().await;

    let made = call(
        &c,
        &backend,
        "/m/create_product",
        json!({ "sku": "ABC-123", "name": "Widget" }),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    assert_eq!(made.body, json!({ "sku": "ABC-123", "name": "Widget" }));

    // Get-by-key keys on the nominated column and returns the same row.
    let got = call(
        &c,
        &backend,
        "/q/product_by_sku",
        json!({ "sku": "ABC-123" }),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body, json!({ "sku": "ABC-123", "name": "Widget" }));
}

#[tokio::test]
async fn fk_and_to_many_nest_correlate_on_the_natural_key() {
    // A relation into a `@key` model carries the key value in its FK column; a to-many nest
    // on the `@key` parent correlates the child FK to the parent's natural key.
    let (c, backend) = key_backend().await;

    call(
        &c,
        &backend,
        "/m/create_product",
        json!({ "sku": "ABC-123", "name": "Widget" }),
    )
    .await;

    let review = call(
        &c,
        &backend,
        "/m/create_review",
        json!({ "product": "ABC-123", "body": "Solid." }),
    )
    .await;
    assert_eq!(review.status, 200, "{:?}", review.body);
    assert_eq!(review.body["body"], json!("Solid."));
    assert_eq!(review.body["product"], json!("ABC-123"));

    // The to-many nest reads the child rows back, correlated on the parent's `sku`.
    let with = call(
        &c,
        &backend,
        "/q/product_reviews",
        json!({ "sku": "ABC-123" }),
    )
    .await;
    assert_eq!(with.status, 200, "{:?}", with.body);
    assert_eq!(with.body["sku"], json!("ABC-123"));
    assert_eq!(with.body["reviews"], json!([{ "body": "Solid." }]));
}
