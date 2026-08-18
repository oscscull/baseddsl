//! End-to-end proof of `list distinct` against a **real** SQLite engine — no mock.
//!
//! `list distinct` lowers to `SELECT DISTINCT` and suppresses the automatic sort cascade
//! (only an explicit `order` applies, so an injected key column can't defeat the dedup).
//! This runs the verbatim `based gen sql` DDL + the runtime's lowered DML against a live
//! in-memory SQLite database: seeding rows with duplicate category values, a plain `list`
//! returns every row (duplicates included) while the `distinct` twin returns each category
//! exactly once, ordered — so a passing run means the dedup actually executes in the DB.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Product { id: Id, name: text, category: text }

shape CategoryName from Product { category }

# every matching row, duplicates included
query all_categories() -> CategoryName[] {
  list Product order (category);
}

# each distinct category exactly once
query distinct_categories() -> CategoryName[] {
  list distinct Product order (category);
}
"#;

// Five products across two categories (three `tools`, two `food`).
const SEED: &str = r#"
INSERT INTO `product` (`id`, `name`, `category`) VALUES ('p1', 'Widget', 'tools');
INSERT INTO `product` (`id`, `name`, `category`) VALUES ('p2', 'Hammer', 'tools');
INSERT INTO `product` (`id`, `name`, `category`) VALUES ('p3', 'Apple', 'food');
INSERT INTO `product` (`id`, `name`, `category`) VALUES ('p4', 'Banana', 'food');
INSERT INTO `product` (`id`, `name`, `category`) VALUES ('p5', 'Nail', 'tools');
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
async fn distinct_dedups_projected_rows_in_the_db() {
    let (c, backend) = backend().await;

    // Plain `list`: every row, duplicates included, ordered by category.
    let all = call(&c, &backend, "/q/all_categories", json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        all.body,
        json!([
            { "category": "food" },
            { "category": "food" },
            { "category": "tools" },
            { "category": "tools" },
            { "category": "tools" },
        ])
    );

    // `list distinct`: each category exactly once — the DB deduped the projected rows.
    let distinct = call(&c, &backend, "/q/distinct_categories", json!({})).await;
    assert_eq!(distinct.status, 200, "{:?}", distinct.body);
    assert_eq!(
        distinct.body,
        json!([{ "category": "food" }, { "category": "tools" }])
    );
}
