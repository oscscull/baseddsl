//! `~ all $patterns` / `~ any $patterns` against a real SQLite engine: the list param expands
//! to one bind per pattern inside `json_each(json_array(…))`. Covers many / one / empty lists,
//! the `not (… ~ any …)` exclusion, a NULL column, the optional `?` param, the signature
//! binding form, and a list passed through a named filter.

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::idempotency::NoStore;
use based_runtime::{dispatch, Compiled, Guards, SeqIdGen};
use crate::driver::SqliteBackend;
use based_sema::check;

const SCHEMA: &str = r#"
Card {
  id: Id
  name: text
  body: text?
  @index(name)
}

shape CardRow from Card { id, name }

filter search(include, exclude) = body ~ all $include and not (body ~ any $exclude);

query all_of(terms: text[]) -> CardRow[] { list Card where (body ~ all $terms) order (name); }
query any_of(terms: text[]) -> CardRow[] { list Card where (body ~ any $terms) order (name); }
query none_of(terms: text[]) -> CardRow[] { list Card where (not (body ~ any $terms)) order (name); }
query all_of_opt(terms?: text[]) -> CardRow[] { list Card where (body ~ all $terms) order (name); }
query any_of_bind(terms: text[] ~ any body) -> CardRow[] order (name);
query searched(include: text[], exclude: text[]) -> CardRow[] {
  list Card where (search($include, $exclude)) order (name);
}
"#;

const SEED: &str = r#"
INSERT INTO `card` (`id`, `name`, `body`) VALUES ('c1', 'Bolt', 'deal 3 damage to any target');
INSERT INTO `card` (`id`, `name`, `body`) VALUES ('c2', 'Counter', 'counter target spell');
INSERT INTO `card` (`id`, `name`, `body`) VALUES ('c3', 'Growth', 'target creature gets +3/+3');
INSERT INTO `card` (`id`, `name`, `body`) VALUES ('c4', 'Shock', 'deal 2 damage to any target');
INSERT INTO `card` (`id`, `name`, `body`) VALUES ('c5', 'Vanilla', NULL);
"#;

const EVERY: [&str; 5] = ["Bolt", "Counter", "Growth", "Shock", "Vanilla"];

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

async fn names(path: &str, args: serde_json::Value) -> Vec<String> {
    let (c, backend) = backend().await;
    let ids = SeqIdGen::default();
    let r = dispatch(
        &c,
        &backend,
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
    .await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    r.body
        .as_array()
        .expect("array body")
        .iter()
        .map(|row| row["name"].as_str().expect("name").to_string())
        .collect()
}

#[tokio::test]
async fn all_requires_every_pattern() {
    let got = names("/q/all_of", json!({ "terms": ["%damage%", "%any target%"] })).await;
    assert_eq!(got, ["Bolt", "Shock"]);
    let got = names("/q/all_of", json!({ "terms": ["%target%"] })).await;
    assert_eq!(got, ["Bolt", "Counter", "Growth", "Shock"]);
}

#[tokio::test]
async fn all_of_an_empty_list_matches_every_row() {
    assert_eq!(names("/q/all_of", json!({ "terms": [] })).await, EVERY);
}

#[tokio::test]
async fn any_requires_one_pattern() {
    let got = names("/q/any_of", json!({ "terms": ["%counter%", "%creature%"] })).await;
    assert_eq!(got, ["Counter", "Growth"]);
}

#[tokio::test]
async fn any_of_an_empty_list_matches_no_row() {
    assert!(names("/q/any_of", json!({ "terms": [] })).await.is_empty());
}

#[tokio::test]
async fn not_any_excludes_matches_and_keeps_null_columns() {
    let got = names("/q/none_of", json!({ "terms": ["%damage%"] })).await;
    assert_eq!(got, ["Counter", "Growth", "Vanilla"]);
    assert_eq!(names("/q/none_of", json!({ "terms": [] })).await, EVERY);
}

#[tokio::test]
async fn optional_list_absent_drops_the_predicate() {
    assert_eq!(names("/q/all_of_opt", json!({})).await, EVERY);
    let got = names("/q/all_of_opt", json!({ "terms": ["%3%"] })).await;
    assert_eq!(got, ["Bolt", "Growth"]);
}

#[tokio::test]
async fn signature_binding_matches_any_pattern() {
    let got = names("/q/any_of_bind", json!({ "terms": ["%spell%"] })).await;
    assert_eq!(got, ["Counter"]);
}

#[tokio::test]
async fn lists_pass_through_a_named_filter() {
    let got = names(
        "/q/searched",
        json!({ "include": ["%target%"], "exclude": ["%damage%"] }),
    )
    .await;
    assert_eq!(got, ["Counter", "Growth"]);
}
