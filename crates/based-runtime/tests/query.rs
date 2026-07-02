//! Runtime read-path tests: request (JSON args + `$ctx`) → bound positional
//! statement + shaped JSON response. Each test parses + checks a whole-schema
//! snippet into a `Compiled`, then drives `plan_query` / `run_query`.
//!
//! The headline assertions: (1) named `:param` / `:ctx_*` placeholders bind to
//! positional `?` with the values in SQL order; (2) input validation rejects a
//! missing/mistyped arg *before* SQL; (3) the response envelope follows the
//! inferred verb/pagination (`get` → object/null, `list` → array, paginated →
//! `{ rows, cursor }`).

use based_ast::FileId;
use based_parser::parse_file;
use based_sema::check;
use serde_json::json;

use based_runtime::plan::{Envelope, PlanError};
use based_runtime::value::{Family, SqlValue};
use based_runtime::{plan_query, run_query, Compiled, MockDb, Request};

/// Compile a whole-schema snippet into a served `Compiled`, asserting it is clean.
fn compile(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls)
}

fn req(name: &str, args: serde_json::Value) -> Request {
    Request::new(name, args, json!({}))
}

fn row(pairs: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    pairs.as_object().cloned().unwrap()
}

const SCHEMA: &str = r#"
    @soft_delete(deleted_at)
    Org { deleted_at: timestamp?, name: text }
    @soft_delete(deleted_at)
    @sort(placed_at desc)
    Order {
        deleted_at: timestamp?,
        org: Org,
        status: text,
        total: int,
        placed_at: timestamp,
    }
    shape OrderCard from Order { status, total }

    query order_by_id(id) -> OrderCard;
    query orders_in_org(org) -> OrderCard[];
    query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }
"#;

#[test]
fn get_binds_param_positionally() {
    let c = compile(SCHEMA);
    let plan = plan_query(&c, &req("order_by_id", json!({ "id": "o-1" }))).unwrap();

    // `:id` became `?`, bound to the arg value; envelope is Option (get).
    assert!(
        plan.main.sql.contains("WHERE `order`.`id` = ?"),
        "{}",
        plan.main.sql
    );
    assert!(
        !plan.main.sql.contains(':'),
        "no named binds left: {}",
        plan.main.sql
    );
    assert_eq!(plan.main.params, vec![SqlValue::Text("o-1".into())]);
    assert_eq!(plan.envelope, Envelope::One);
    assert!(plan.count.is_none());
}

#[test]
fn get_shapes_single_row_as_option() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
    let out = run_query(&c, &mut db, &req("order_by_id", json!({ "id": "o-1" }))).unwrap();
    assert_eq!(out, json!({ "status": "paid", "total": 42 }));
}

#[test]
fn get_missing_row_is_json_null() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![vec![]]);
    let out = run_query(&c, &mut db, &req("order_by_id", json!({ "id": "nope" }))).unwrap();
    assert_eq!(out, serde_json::Value::Null);
}

#[test]
fn list_shapes_rows_as_array() {
    let c = compile(SCHEMA);
    let plan = plan_query(&c, &req("orders_in_org", json!({ "org": "org-9" }))).unwrap();
    assert_eq!(plan.envelope, Envelope::Many);
    assert_eq!(plan.main.params, vec![SqlValue::Text("org-9".into())]);

    let mut db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 1 })),
        row(json!({ "status": "pending", "total": 2 })),
    ]]);
    let out = run_query(
        &c,
        &mut db,
        &req("orders_in_org", json!({ "org": "org-9" })),
    )
    .unwrap();
    assert_eq!(
        out,
        json!([
            { "status": "paid", "total": 1 },
            { "status": "pending", "total": 2 }
        ])
    );
}

#[test]
fn ctx_field_binds_from_request_context() {
    let c = compile(SCHEMA);
    // `$ctx.org` renders `:ctx_org`; it must bind from the request context, not args.
    let r = Request::new("my_org_orders", json!({}), json!({ "org": "org-7" }));
    let plan = plan_query(&c, &r).unwrap();
    assert!(
        plan.main.sql.contains("WHERE `order`.`org_id` = ?"),
        "{}",
        plan.main.sql
    );
    assert_eq!(plan.main.params, vec![SqlValue::Text("org-7".into())]);
}

#[test]
fn missing_ctx_is_rejected() {
    let c = compile(SCHEMA);
    let err = plan_query(&c, &req("my_org_orders", json!({}))).unwrap_err();
    assert_eq!(err, PlanError::MissingCtx("org".into()));
}

#[test]
fn missing_required_arg_is_rejected() {
    let c = compile(SCHEMA);
    let err = plan_query(&c, &req("order_by_id", json!({}))).unwrap_err();
    assert_eq!(err, PlanError::MissingArg("id".into()));
}

#[test]
fn wrong_typed_arg_is_rejected_before_sql() {
    // `total: int` param, but the caller sends a string.
    let c = compile(
        r#"
        Product { name: text, price: int }
        shape Card from Product { name }
        query priced(price: int) -> Card[] { list Product where (price > $price); }
        "#,
    );
    let err = plan_query(&c, &req("priced", json!({ "price": "lots" }))).unwrap_err();
    assert_eq!(
        err,
        PlanError::BadArg {
            name: "price".into(),
            expected: Family::Int,
            got: "string".into(),
        }
    );
}

#[test]
fn default_applied_when_arg_omitted() {
    let c = compile(
        r#"
        Product { name: text, active: bool }
        shape Card from Product { name }
        query listing(active: bool = true) -> Card[] { list Product where (active = $active); }
        "#,
    );
    let plan = plan_query(&c, &req("listing", json!({}))).unwrap();
    assert_eq!(plan.main.params, vec![SqlValue::Bool(true)]);
}

#[test]
fn unknown_callable_is_rejected() {
    let c = compile(SCHEMA);
    let err = plan_query(&c, &req("nope", json!({}))).unwrap_err();
    assert_eq!(err, PlanError::UnknownQuery("nope".into()));
}

#[test]
fn paginated_list_uses_page_envelope_with_offset_and_count() {
    let c = compile(
        r#"
        @sort(created_at desc)
        Product { created_at: timestamp, name: text }
        shape Card from Product { name }
        query recent() -> Card[] { list Product page (20) offset with count; }
        "#,
    );
    let plan = plan_query(&c, &req("recent", json!({ "offset": 40 }))).unwrap();
    assert_eq!(plan.envelope, Envelope::Page { with_count: true });
    assert!(
        plan.main.sql.contains("LIMIT 20 OFFSET ?"),
        "{}",
        plan.main.sql
    );
    assert_eq!(plan.main.params, vec![SqlValue::Int(40)]);
    assert!(plan.count.is_some());

    // Row batch then count batch → { rows, cursor, total }.
    let mut db = MockDb::new(vec![
        vec![row(json!({ "name": "a" }))],
        vec![row(json!({ "count": 57 }))],
    ]);
    let out = run_query(&c, &mut db, &req("recent", json!({ "offset": 40 }))).unwrap();
    assert_eq!(
        out,
        json!({ "rows": [{ "name": "a" }], "cursor": null, "total": 57 })
    );
}

#[test]
fn offset_defaults_to_zero() {
    let c = compile(
        r#"
        @sort(created_at desc)
        Product { created_at: timestamp, name: text }
        shape Card from Product { name }
        query recent() -> Card[] { list Product page (20) offset; }
        "#,
    );
    let plan = plan_query(&c, &req("recent", json!({}))).unwrap();
    assert_eq!(plan.main.params, vec![SqlValue::Int(0)]);
}
