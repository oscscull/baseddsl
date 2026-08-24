//! End-to-end proof that a `raw` query body's verbatim SQL — including a non-ASCII
//! string literal — executes byte-for-byte against a **real** SQLite engine, no mock.
//!
//! A whole-query `raw` block is emitted into the SQL template verbatim; the runtime then
//! rewrites `:name` placeholders to positional binds before executing. That rewrite must
//! copy the surrounding SQL text intact — a multi-byte UTF-8 literal (`'café'`, `'🎉'`)
//! has to survive so it still matches the seeded row. Seeding a product named `café` and
//! filtering on that literal through a raw body returns exactly that row: a passing run
//! means the literal was not corrupted on the way to the driver.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Product { id: Id, name: text, note: text }

shape ProductRow from Product { id, name }

# A whole-query raw body carrying a non-ASCII literal in its WHERE, plus a bound param.
query products_named_cafe(min: text) -> ProductRow[] {
  raw`SELECT id AS id, name AS name
      FROM {table}
      WHERE name = 'café' AND id >= ${min}`;
}
"#;

const SEED: &str = r#"
INSERT INTO `product` (`id`, `name`, `note`) VALUES ('p1', 'café', 'accented ☕');
INSERT INTO `product` (`id`, `name`, `note`) VALUES ('p2', 'cafe', 'plain ascii');
INSERT INTO `product` (`id`, `name`, `note`) VALUES ('p3', 'tea', 'unrelated');
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
async fn raw_body_non_ascii_literal_matches_the_seeded_row() {
    let (c, backend) = backend().await;

    // The raw body filters on the literal `'café'`; only p1 matches. If the runtime
    // corrupted the UTF-8 literal into mojibake, no row would match and this is `[]`.
    let resp = call(
        &c,
        &backend,
        "/q/products_named_cafe",
        json!({ "min": "p0" }),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!([{ "id": "p1", "name": "café" }]));
}
