//! Exercises the disk front end (`Compiled::load`) against the real commerce
//! example, then plans a couple of its queries end-to-end — proving the loader,
//! the `$ctx` inference path, and binding all line up on a non-toy schema.

use std::path::PathBuf;

use serde_json::json;

use based_runtime::value::SqlValue;
use based_runtime::{plan_mutation, plan_query, Compiled, Request, SeqIdGen};

fn commerce() -> Compiled {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spec/examples/commerce")
        .canonicalize()
        .expect("commerce example dir");
    Compiled::load(&root).unwrap_or_else(|e| panic!("commerce did not load: {e:?}"))
}

const SQLITE: &str = "dialect = \"sqlite\"\n[schema]\nid = \"serial\"\n";
const TASK: &str = "Task {\n  id: Id\n  title: text\n}\n\nquery tasks() -> Task[] { list Task; }\n";

#[test]
fn loads_embedded_sources_with_the_manifest_settings() {
    let c = Compiled::from_sources(SQLITE, &[("schema/task.bsl", TASK)])
        .unwrap_or_else(|e| panic!("embedded schema did not load: {e}"));
    assert_eq!(c.dialect, based_codegen::Dialect::Sqlite);
    assert!(c.queries.contains_key("tasks"));
}

#[test]
fn embedded_diagnostics_name_the_embedded_file() {
    let broken = "Note {\n  id: Id\n  owner: Nope\n}\n";
    let e = match Compiled::from_sources(SQLITE, &[("task.bsl", TASK), ("broken.bsl", broken)]) {
        Ok(_) => panic!("a broken schema must not load"),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("broken.bsl:3:"), "{e}");
}

#[test]
fn embedded_manifest_errors_block() {
    assert!(Compiled::from_sources("dialect = \"oracle\"", &[("task.bsl", TASK)]).is_err());
    assert!(Compiled::from_sources(SQLITE, &[]).is_err());
}

#[test]
fn loads_and_lowers_commerce() {
    let c = commerce();
    // The example's queries are all present and lowered.
    for q in [
        "order_by_id",
        "orders_in_org",
        "my_org_orders",
        "active_products",
    ] {
        assert!(c.queries.contains_key(q), "missing lowered query {q}");
    }
}

#[test]
fn plans_a_commerce_ctx_query() {
    let c = commerce();
    // `my_org_orders` is a plain `list Order`, but Order is `@scope`d  so the org
    // filter is injected from `$ctx` — it still binds from context, positionally.
    let r = Request::new("my_org_orders", json!({}), json!({ "org": "org-42" }));
    let plan = plan_query(&c, &r).unwrap();
    assert!(
        plan.main.sql.contains("`order`.`org_id` = ?"),
        "{}",
        plan.main.sql
    );
    assert_eq!(plan.main.params, vec![SqlValue::Uuid("org-42".into())]);
}

#[test]
fn plans_the_commerce_place_order_mutation() {
    let c = commerce();
    // `place_order` creates an Order; the engine generates its id, and the response
    // identifies that row (return model = Order). Order is `@scope`d, so `org` comes
    // from `$ctx` (auto-set on create) — never a body arg.
    let ids = SeqIdGen::default();
    let r = Request::new(
        "place_order",
        json!({ "buyer": "user-1", "total": "99.00" }),
        json!({ "org": "org-1" }),
    );
    let plan = plan_mutation(&c, &r, &ids).unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert!(
        plan.steps[0].sql.contains("INSERT INTO `order`"),
        "{}",
        plan.steps[0].sql
    );
    // Steps hold unbound `:name` SQL (bound late at run time); the app-minted engine id
    // is in the plan environment and drives the response identity.
    assert_eq!(
        plan.env0.get("id").cloned(),
        Some(SqlValue::Uuid("id-0".into()))
    );
    assert_eq!(plan.result_id.as_deref(), Some("id-0"));
    // A plain (unbound) create needs no row read-back.
    assert!(plan.steps[0].capture.is_none());
}
