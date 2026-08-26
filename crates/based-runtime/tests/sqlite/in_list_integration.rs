//! End-to-end proof of variable-length `col in $arr` against a **real** SQLite engine — the
//! path from issue #10, which `based check` accepted but that 400'd at runtime for any array
//! param. An array-typed param (`text[]`, `int[]`) binds to a bind list that expands to
//! `col IN (?, ?, …)` — one placeholder per element — so a runtime-sized set filters live rows.
//! Covers the two lowering sites (a `where (col in $arr)` predicate and a `$arr in col` param
//! binding), the empty array (→ `IN (NULL)`, matches nothing, never the `IN ()` syntax error),
//! the optional `in $arr?` composing with the present-guard, per-element family coercion
//! (`int[]`), and the boundary errors (a non-array arg, a wrong-typed element).

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen, SqliteBackend};
use based_sema::check;

const SCHEMA: &str = r#"
Card {
  id: Id
  name: text
  color_identity: text
  rank: int
  @index(name)
  @index(color_identity)
  @index(rank)
}

shape CardRow from Card { id, name, color_identity, rank }

# `where (col in $arr)` — the exact repro (a plain `Cmp` with `in`, single-placeholder template)
query find_in(vals: text[]) -> CardRow[] { list Card where (color_identity in $vals) order (name); }

# optional `in $arr?` — composes with the present-guard
query find_in_opt(vals?: text[]) -> CardRow[] { list Card where (color_identity in $vals) order (name); }

# param-binding form `$arr in col` — the other lowering site (`param_condition`)
query find_in_bind(vals: text[] in color_identity) -> CardRow[] order (name);

# per-element coercion against a non-text scalar family (`int`)
query find_ranks(ranks: int[]) -> CardRow[] { list Card where (rank in $ranks) order (name); }
"#;

// Five cards: two red (R), two blue (U), one green (G); ascending ranks.
const SEED: &str = r#"
INSERT INTO `card` (`id`, `name`, `color_identity`, `rank`) VALUES ('c1', 'Bolt', 'R', 1);
INSERT INTO `card` (`id`, `name`, `color_identity`, `rank`) VALUES ('c2', 'Counter', 'U', 2);
INSERT INTO `card` (`id`, `name`, `color_identity`, `rank`) VALUES ('c3', 'Growth', 'G', 3);
INSERT INTO `card` (`id`, `name`, `color_identity`, `rank`) VALUES ('c4', 'Shock', 'R', 4);
INSERT INTO `card` (`id`, `name`, `color_identity`, `rank`) VALUES ('c5', 'Brainstorm', 'U', 5);
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
async fn in_list_matches_the_bound_values() {
    let (c, backend) = backend().await;
    // The repro: bind ["R", "U"] → every red and blue card.
    let r = call(&c, &backend, "/q/find_in", json!({ "vals": ["R", "U"] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Bolt", "Brainstorm", "Counter", "Shock"]);
}

#[tokio::test]
async fn in_list_single_element() {
    let (c, backend) = backend().await;
    let r = call(&c, &backend, "/q/find_in", json!({ "vals": ["G"] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Growth"]);
}

#[tokio::test]
async fn empty_in_list_matches_nothing() {
    let (c, backend) = backend().await;
    // An empty array lowers to `col IN (NULL)` — a legal statement that matches no row (never
    // the `IN ()` syntax error).
    let r = call(&c, &backend, "/q/find_in", json!({ "vals": [] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert!(names(&r.body).is_empty(), "{:?}", r.body);
}

#[tokio::test]
async fn in_list_via_param_binding() {
    let (c, backend) = backend().await;
    // The `$arr in col` binding form lowers the same expandable template.
    let r = call(&c, &backend, "/q/find_in_bind", json!({ "vals": ["U"] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Brainstorm", "Counter"]);
}

#[tokio::test]
async fn optional_in_list_absent_drops_the_predicate() {
    let (c, backend) = backend().await;
    // `vals` omitted → the present-guard drops the `in` leaf → every card.
    let r = call(&c, &backend, "/q/find_in_opt", json!({})).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(
        names(&r.body),
        ["Bolt", "Brainstorm", "Counter", "Growth", "Shock"]
    );
}

#[tokio::test]
async fn optional_in_list_present_applies() {
    let (c, backend) = backend().await;
    // Present → the guard's flag is 1, the list expands: only red cards.
    let r = call(&c, &backend, "/q/find_in_opt", json!({ "vals": ["R"] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Bolt", "Shock"]);
}

#[tokio::test]
async fn optional_empty_in_list_present_matches_nothing() {
    let (c, backend) = backend().await;
    // Present but empty → present=1, `col IN (NULL)` → no row (distinct from absent).
    let r = call(&c, &backend, "/q/find_in_opt", json!({ "vals": [] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert!(names(&r.body).is_empty(), "{:?}", r.body);
}

#[tokio::test]
async fn in_list_coerces_each_element_by_family() {
    let (c, backend) = backend().await;
    // `int[]` — each element binds as an int, not text.
    let r = call(&c, &backend, "/q/find_ranks", json!({ "ranks": [1, 4] })).await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(names(&r.body), ["Bolt", "Shock"]);
}

#[tokio::test]
async fn non_array_arg_for_array_param_is_bad_arg() {
    let (c, backend) = backend().await;
    // A scalar where an array is expected is a boundary error (400), not a silent match.
    let r = call(&c, &backend, "/q/find_in", json!({ "vals": "R" })).await;
    assert_eq!(r.status, 400, "{:?}", r.body);
    assert_eq!(r.body["error"], "bad_arg");
}

#[tokio::test]
async fn wrong_typed_element_is_bad_arg() {
    let (c, backend) = backend().await;
    // A non-text element in a `text[]` list fails coercion at the boundary (400).
    let r = call(&c, &backend, "/q/find_in", json!({ "vals": ["R", 5] })).await;
    assert_eq!(r.status, 400, "{:?}", r.body);
    assert_eq!(r.body["error"], "bad_arg");
}
