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
    assert_eq!(plan.stmts.len(), 1);
    assert!(
        plan.stmts[0].sql.contains("INSERT INTO `order`"),
        "{}",
        plan.stmts[0].sql
    );
    // engine id leads the bound values; params carry no unresolved `:name`.
    assert!(!plan.stmts[0].sql.contains(':'), "{}", plan.stmts[0].sql);
    assert_eq!(plan.stmts[0].params[0], SqlValue::Uuid("id-0".into()));
    assert_eq!(plan.result_id.as_deref(), Some("id-0"));
}
