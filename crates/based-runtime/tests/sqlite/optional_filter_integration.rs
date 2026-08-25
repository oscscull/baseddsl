//! End-to-end proof of `?` optional filter params against a **real** SQLite engine — no
//! mock. A `?` param is a two-state filter (queries.md): absent drops the predicate, a value
//! applies it — with ANY operator, not just equality. This runs the verbatim `based gen sql`
//! DDL + the runtime's lowered present-guarded predicate against a live in-memory database, so
//! a passing run means the skip/apply behavior actually executes in the DB — for equality, a
//! `~` LIKE, a `>` range, and two optional params composing (one skipped, one applied).

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
  rank: int
  status: text?
  @index(name)
  @index(status)
  @index(category)
  @index(rank)
}

shape ProductName from Product { name }

# one optional equality filter
query search(status?) -> ProductName[] order (name);

# two optional filters compose with AND; either may be skipped independently
query search2(status?, category?) -> ProductName[] order (name);

# optional non-equality filters — the generalized capability
query search_like(pat?: text ~ name) -> ProductName[] order (name);
query search_gt(min?: int > rank) -> ProductName[] order (name);

# block `where` + or-composition: each `?` leaf present-guards independently, so an absent
# arg widens just its branch (an absent `q` makes the whole or-group TRUE).
query search_or(q?, min?) -> ProductName[] { list Product where ((name ~ $q or category ~ $q) and rank >= $min) order (name); }
"#;

// Five products across two categories, with ascending ranks; two carry a NULL status.
const SEED: &str = r#"
INSERT INTO `product` (`id`, `name`, `category`, `rank`, `status`) VALUES ('p1', 'Widget', 'tools', 10, 'active');
INSERT INTO `product` (`id`, `name`, `category`, `rank`, `status`) VALUES ('p2', 'Hammer', 'tools', 20, 'active');
INSERT INTO `product` (`id`, `name`, `category`, `rank`, `status`) VALUES ('p3', 'Apple', 'food', 30, NULL);
INSERT INTO `product` (`id`, `name`, `category`, `rank`, `status`) VALUES ('p4', 'Banana', 'food', 40, NULL);
INSERT INTO `product` (`id`, `name`, `category`, `rank`, `status`) VALUES ('p5', 'Nail', 'tools', 50, 'shipped');
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
async fn value_optional_filter_matches_equality() {
    let (c, backend) = backend().await;
    // A value → ordinary equality, NULL rows excluded.
    let r = call(&c, &backend, "/q/search", json!({ "status": "active" })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Hammer", "Widget"]);
}

#[tokio::test]
async fn explicit_null_value_matches_nothing_under_2state() {
    let (c, backend) = backend().await;
    // 2-state: null is no longer a param state. The client's `Option<T>` never sends null
    // (None omits the field); a raw caller that sends an explicit `null` gets present=1 with
    // a NULL value → `status = NULL` → matches nothing. Null-matching lives in the body now.
    let r = call(&c, &backend, "/q/search", json!({ "status": null })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert!(names(&r.body).is_empty(), "{:?}", r.body);
}

#[tokio::test]
async fn optional_like_filter_skips_or_matches() {
    let (c, backend) = backend().await;
    // Absent → every row.
    let all = call(&c, &backend, "/q/search_like", json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        names(&all.body),
        ["Apple", "Banana", "Hammer", "Nail", "Widget"]
    );
    // Present → `name LIKE 'Ha%'` → just Hammer.
    let hit = call(&c, &backend, "/q/search_like", json!({ "pat": "Ha%" })).await;
    assert_eq!(hit.status, 200, "{:?}", hit.body);
    assert_eq!(names(&hit.body), ["Hammer"]);
}

#[tokio::test]
async fn optional_range_filter_skips_or_matches() {
    let (c, backend) = backend().await;
    // Absent → every row.
    let all = call(&c, &backend, "/q/search_gt", json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(all.body.as_array().expect("array").len(), 5);
    // Present → `rank > 25` → Apple(30), Banana(40), Nail(50).
    let hit = call(&c, &backend, "/q/search_gt", json!({ "min": 25 })).await;
    assert_eq!(hit.status, 200, "{:?}", hit.body);
    assert_eq!(names(&hit.body), ["Apple", "Banana", "Nail"]);
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

    // A different value + a category: shipped AND tools → just Nail.
    let shipped_tools = call(
        &c,
        &backend,
        "/q/search2",
        json!({ "status": "shipped", "category": "tools" }),
    )
    .await;
    assert_eq!(shipped_tools.status, 200, "{:?}", shipped_tools.body);
    assert_eq!(names(&shipped_tools.body), ["Nail"]);
}

#[tokio::test]
async fn block_or_composition_skips_and_applies() {
    let (c, backend) = backend().await;

    // Both absent → the or-group and the range both widen → every row.
    let all = call(&c, &backend, "/q/search_or", json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        names(&all.body),
        ["Apple", "Banana", "Hammer", "Nail", "Widget"]
    );

    // `q` present → `name LIKE 'tool%' OR category LIKE 'tool%'` — category "tools" matches the
    // three tools rows; the range widens (min absent).
    let tools = call(&c, &backend, "/q/search_or", json!({ "q": "tool%" })).await;
    assert_eq!(tools.status, 200, "{:?}", tools.body);
    assert_eq!(names(&tools.body), ["Hammer", "Nail", "Widget"]);

    // `min` present → `rank >= 30` applies; the or-group widens (q absent).
    let min = call(&c, &backend, "/q/search_or", json!({ "min": 30 })).await;
    assert_eq!(min.status, 200, "{:?}", min.body);
    assert_eq!(names(&min.body), ["Apple", "Banana", "Nail"]);

    // Both → (name/category LIKE 'tool%') AND rank >= 30 → only Nail (tools, rank 50).
    let both = call(
        &c,
        &backend,
        "/q/search_or",
        json!({ "q": "tool%", "min": 30 }),
    )
    .await;
    assert_eq!(both.status, 200, "{:?}", both.body);
    assert_eq!(names(&both.body), ["Nail"]);
}
