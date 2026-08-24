//! End-to-end proof of computed shape fields against a **real** SQLite engine — no mock.
//!
//! A computed shape field (`out = <expr>`) projects a per-row derived scalar: arithmetic
//! over the numeric family, string concatenation, and a `case … when … then … else … end`
//! conditional. This runs the verbatim `based gen sql` DDL + the runtime's lowered DML
//! against a live in-memory SQLite database, seeding rows and asserting the derived values
//! come back computed **server-side** — so a passing run means the expression executed in
//! the database, not in Rust.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Product {
  id:       Id
  name:     text
  brand:    text
  price:    int
  discount: int
}

shape ProductCard from Product {
  name
  net   = price - discount
  gross = (price - discount) * 2
  label = brand || " " || name
  tier  = case when price > 100 then "premium" else "standard" end
}

query product_card(id) -> ProductCard;
query all_cards() -> ProductCard[] { list Product order (name); }
"#;

const SEED: &str = r#"
INSERT INTO `product` (`id`, `name`, `brand`, `price`, `discount`) VALUES ('p1', 'Widget', 'Acme', 150, 30);
INSERT INTO `product` (`id`, `name`, `brand`, `price`, `discount`) VALUES ('p2', 'Bolt', 'Hardware', 40, 5);
"#;

async fn backend() -> (Compiled, SqliteBackend) {
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
        .unwrap_or_else(|e| panic!("DDL failed to execute: {e:?}\n{ddl}"));
    backend.execute_batch(SEED).await.expect("seed");
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
async fn computed_fields_are_evaluated_server_side() {
    let (c, backend) = backend().await;

    // p1: price 150, discount 30 → net 120, gross 240, label "Acme Widget", tier "premium".
    let one = call(&c, &backend, "/q/product_card", json!({ "id": "p1" })).await;
    assert_eq!(one.status, 200, "{:?}", one.body);
    assert_eq!(
        one.body,
        json!({
            "name": "Widget",
            "net": 120,
            "gross": 240,
            "label": "Acme Widget",
            "tier": "premium",
        })
    );

    // The full list proves the CASE branches diverge per row and concat/arith hold.
    let all = call(&c, &backend, "/q/all_cards", json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        all.body,
        json!([
            {
                "name": "Bolt",
                "net": 35,
                "gross": 70,
                "label": "Hardware Bolt",
                "tier": "standard",
            },
            {
                "name": "Widget",
                "net": 120,
                "gross": 240,
                "label": "Acme Widget",
                "tier": "premium",
            },
        ])
    );
}
