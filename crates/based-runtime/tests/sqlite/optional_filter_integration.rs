//! End-to-end proof of `?` optional filter params against a **real** SQLite engine — no
//! mock. A `status?` param is a three-state filter (queries.md): absent drops the predicate,
//! a JSON `null` matches `col IS NULL`, a value matches equality. This runs the verbatim
//! `based gen sql` DDL + the runtime's lowered guarded predicate against a live in-memory
//! database, so a passing run means the drop / `IS NULL` / equality behavior actually
//! executes in the DB — including two optional params composing (one skipped, one applied).

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
  id: Id
  name: text
  category: text
  status: text?
  @index(status)
  @index(category)
}

shape ProductName from Product { name }

# one optional filter, three states
query search(status?) -> ProductName[] order (name);

# two optional filters compose with AND; either may be skipped independently
query search2(status?, category?) -> ProductName[] order (name);
"#;

// Five products: two `active`, one `shipped`, two with NULL status; across two categories.
const SEED: &str = r#"
INSERT INTO `product` (`id`, `name`, `category`, `status`) VALUES ('p1', 'Widget', 'tools', 'active');
INSERT INTO `product` (`id`, `name`, `category`, `status`) VALUES ('p2', 'Hammer', 'tools', 'active');
INSERT INTO `product` (`id`, `name`, `category`, `status`) VALUES ('p3', 'Apple', 'food', NULL);
INSERT INTO `product` (`id`, `name`, `category`, `status`) VALUES ('p4', 'Banana', 'food', NULL);
INSERT INTO `product` (`id`, `name`, `category`, `status`) VALUES ('p5', 'Nail', 'tools', 'shipped');
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

fn names(body: &serde_json::Value) -> Vec<String> {
    body.as_array()
        .expect("array body")
        .iter()
        .map(|r| r["name"].as_str().expect("name").to_string())
        .collect()
}

#[tokio::test]
async fn absent_optional_filter_drops_the_predicate() {
    let (c, backend) = backend().await;
    // `status` omitted entirely → no status predicate → every row.
    let r = call(&c, &backend, "/q/search", json!({})).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(
        names(&r.body),
        ["Apple", "Banana", "Hammer", "Nail", "Widget"]
    );
}

#[tokio::test]
async fn null_optional_filter_matches_is_null() {
    let (c, backend) = backend().await;
    // Explicit JSON null → `status IS NULL` → only the two NULL-status rows.
    let r = call(&c, &backend, "/q/search", json!({ "status": null })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Apple", "Banana"]);
}

#[tokio::test]
async fn value_optional_filter_matches_equality() {
    let (c, backend) = backend().await;
    // A value → ordinary equality, NULL rows excluded.
    let r = call(&c, &backend, "/q/search", json!({ "status": "active" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Hammer", "Widget"]);
}

#[tokio::test]
async fn two_optional_filters_compose_and_skip_independently() {
    let (c, backend) = backend().await;

    // Both supplied: active AND in tools → equality-AND.
    let both = call(
        &c,
        &backend,
        "/q/search2",
        json!({ "status": "active", "category": "tools" }),
    )
    .await;
    assert_eq!(both.status, 200, "{:?}", both.body);
    assert_eq!(names(&both.body), ["Hammer", "Widget"]);

    // Only category supplied: status skipped → every tools row (active + shipped).
    let cat_only = call(&c, &backend, "/q/search2", json!({ "category": "tools" })).await;
    assert_eq!(cat_only.status, 200, "{:?}", cat_only.body);
    assert_eq!(names(&cat_only.body), ["Hammer", "Nail", "Widget"]);

    // Null status + a category: `status IS NULL` AND in food.
    let null_and_cat = call(
        &c,
        &backend,
        "/q/search2",
        json!({ "status": null, "category": "food" }),
    )
    .await;
    assert_eq!(null_and_cat.status, 200, "{:?}", null_and_cat.body);
    assert_eq!(names(&null_and_cat.body), ["Apple", "Banana"]);
}
