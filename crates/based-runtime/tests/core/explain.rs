//! `explain` reports exactly the statements and bind order execution uses.

use based_ast::FileId;
use based_codegen::Dialect;
use based_parser::parse_file;
use based_runtime::explain::{explain, explain_all, CallableKind};
use based_runtime::{plan_mutation, plan_query, Compiled, Request, SeqIdGen};
use serde_json::json;

const SCHEMA: &str = r#"
Item {
  id:     Id
  owner:  text
  status: text
  name:   text
}
query items(status?) -> Item[] { list Item where (owner = $ctx.user and status = $status); }
mutation rename(id: Id, name) -> Item {
  update Item where (id = $id and owner = $ctx.user) { name = $name };
}
mutation add(name) -> Item {
  create Item { name = $name, owner = $ctx.user, status = "new" };
}
"#;

fn compile(dialect: Dialect) -> Compiled {
    let sf = parse_file(SCHEMA, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = based_sema::check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls, dialect)
}

#[test]
fn query_explain_matches_the_planned_statement() {
    for dialect in [Dialect::MariaDb, Dialect::Postgres, Dialect::Sqlite] {
        let compiled = compile(dialect);
        let explained = explain(&compiled, "items");
        assert_eq!(explained.len(), 1);
        assert_eq!(explained[0].kind, CallableKind::Query);
        let stmt = &explained[0].statements[0];
        let req = Request::new("items", json!({ "status": "open" }), json!({ "user": "u" }));
        let plan = plan_query(&compiled, &req).unwrap();
        assert_eq!(stmt.sql, plan.main.sql.trim_end());
        assert_eq!(stmt.params.len(), plan.main.params.len());
        assert!(stmt.params.contains(&"$ctx.user".to_string()), "{stmt:?}");
        assert!(stmt.params.contains(&"$status".to_string()), "{stmt:?}");
        assert!(
            stmt.params.contains(&"$status present (1/0)".to_string()),
            "{stmt:?}"
        );
    }
}

#[test]
fn mutation_explain_lists_every_statement_in_run_order() {
    let compiled = compile(Dialect::Postgres);
    let explained = explain(&compiled, "rename");
    assert_eq!(explained[0].kind, CallableKind::Mutation);
    let req = Request::new(
        "rename",
        json!({ "id": "00000000-0000-0000-0000-000000000001", "name": "n" }),
        json!({ "user": "u" }),
    );
    let plan = plan_mutation(&compiled, &req, &SeqIdGen::default()).unwrap();
    let statements = &explained[0].statements;
    assert_eq!(statements.len(), plan.steps.len() + 1, "{statements:#?}");
    assert!(statements[0].sql.starts_with("UPDATE"), "{statements:#?}");
    assert!(statements[0].sql.contains("$1"), "{statements:#?}");
    assert!(statements.last().unwrap().sql.starts_with("SELECT"));
}

#[test]
fn create_binds_the_engine_minted_id() {
    let compiled = compile(Dialect::Sqlite);
    let create = &explain(&compiled, "add")[0].statements[0];
    assert!(create.sql.starts_with("INSERT"), "{create:?}");
    assert!(
        create.params.contains(&"id (engine)".to_string()),
        "{create:?}"
    );
    assert!(
        create.params.contains(&"$ctx.user".to_string()),
        "{create:?}"
    );
}

#[test]
fn explain_all_covers_every_callable_in_order() {
    let compiled = compile(Dialect::MariaDb);
    let names: Vec<_> = explain_all(&compiled).into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["items", "rename", "add"]);
    assert!(explain(&compiled, "missing").is_empty());
}
