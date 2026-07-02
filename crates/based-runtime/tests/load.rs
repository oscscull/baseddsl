//! Exercises the disk front end (`Compiled::load`) against the real commerce
//! example, then plans a couple of its queries end-to-end — proving the loader,
//! the `$ctx` inference path, and binding all line up on a non-toy schema.

use std::path::PathBuf;

use serde_json::json;

use based_runtime::value::SqlValue;
use based_runtime::{plan_query, Compiled, Request};

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
    // `my_org_orders` reads `$ctx.org` — it must bind from context, positionally.
    let r = Request::new("my_org_orders", json!({}), json!({ "org": "org-42" }));
    let plan = plan_query(&c, &r).unwrap();
    assert!(
        plan.main.sql.contains("WHERE `order`.`org_id` = ?"),
        "{}",
        plan.main.sql
    );
    assert_eq!(plan.main.params, vec![SqlValue::Text("org-42".into())]);
}
