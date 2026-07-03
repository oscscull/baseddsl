//! Wire-dispatch tests: a decoded HTTP request (method, path, JSON args, `$ctx`) →
//! a `WireResponse` (status + JSON body). The whole route → plan → run → response
//! path runs against a `MockDb`, no network and no database.
//!
//! Headline assertions: (1) the route prefix selects query vs mutation and 404s on a
//! bad/mismatched route; (2) success returns 200 + the shaped response; (3) every
//! `PlanError` maps to its HTTP status + `{ error: { code, message } }`; (4) only POST
//! is accepted (no GET query-string surface, calling.md).

use based_ast::FileId;
use based_parser::parse_file;
use based_sema::check;
use serde_json::json;

use based_runtime::{dispatch, Compiled, MockDb, Row, SeqIdGen};

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

fn row(pairs: serde_json::Value) -> Row {
    pairs.as_object().cloned().unwrap()
}

const SCHEMA: &str = r#"
    @soft_delete(deleted_at)
    Org { deleted_at: timestamp?, name: text }
    @soft_delete(deleted_at)
    Order {
        deleted_at: timestamp?,
        org: Org,
        status: text,
        total: int,
    }
    shape OrderCard from Order { status, total }

    query order_by_id(id) -> OrderCard;
    query orders_in_org(org) -> OrderCard[];
    query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }

    mutation place_order(org: Id, status, total: int) -> OrderCard {
        create Order { org = $org, status = $status, total = $total };
    }
"#;

/// A query route runs the query and returns 200 + the shaped response (a `get` → the
/// single object).
#[test]
fn query_route_returns_shaped_response() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "paid", "total": 42 }));
    // The arg bound positionally into the fetched statement.
    assert_eq!(
        db.calls[0].1,
        vec![based_runtime::SqlValue::Text("o-1".into())]
    );
}

/// A `list` route returns an array (envelope Many).
#[test]
fn list_route_returns_array() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 1 })),
        row(json!({ "status": "open", "total": 2 })),
    ]]);
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/orders_in_org",
        json!({ "org": "org-1" }),
        json!({}),
    );
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_array());
    assert_eq!(resp.body.as_array().unwrap().len(), 2);
}

/// A mutation route runs the write path and returns the created row's engine id.
#[test]
fn mutation_route_returns_created_id() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![]);
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "id": "id-0" }));
    // One INSERT executed between begin/commit.
    assert_eq!(db.tx, vec!["begin", "commit"]);
    assert_eq!(db.calls.len(), 1);
    assert!(db.calls[0].0.contains("INSERT INTO `order`"));
}

/// `$ctx` arrives as the server-supplied context (not the body); a required one that
/// is missing is a 400 `missing_ctx`.
#[test]
fn ctx_supplied_out_of_band_and_required() {
    let c = compile(SCHEMA);
    let mut ids = SeqIdGen::default();

    // Provided → 200.
    let mut db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 1 }))]]);
    let ok = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-9" }),
    );
    assert_eq!(ok.status, 200);
    assert_eq!(
        db.calls[0].1,
        vec![based_runtime::SqlValue::Text("org-9".into())]
    );

    // Missing → 400 missing_ctx.
    let mut db2 = MockDb::new(vec![]);
    let miss = dispatch(
        &c,
        &mut db2,
        &mut ids,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({}),
    );
    assert_eq!(miss.status, 400);
    assert_eq!(miss.body["error"]["code"], "missing_ctx");
}

/// A missing required arg is a 400 `missing_arg`; a mistyped arg is a 400 `bad_arg`.
#[test]
fn arg_validation_maps_to_400() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![]);
    let mut ids = SeqIdGen::default();

    let missing = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/order_by_id",
        json!({}),
        json!({}),
    );
    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"]["code"], "missing_arg");

    let bad = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": "not-an-int" }),
        json!({}),
    );
    assert_eq!(bad.status, 400);
    assert_eq!(bad.body["error"]["code"], "bad_arg");
    // No SQL ran — validation is at the boundary.
    assert!(db.calls.is_empty());
}

/// An unknown callable is a 404; the route prefix is authoritative (a mutation name
/// under `/q/` is an unknown *query*, never a cross-dispatch).
#[test]
fn unknown_and_mismatched_routes_404() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![]);
    let mut ids = SeqIdGen::default();

    let unknown = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/nope",
        json!({}),
        json!({}),
    );
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.body["error"]["code"], "unknown_query");

    // `place_order` is a mutation; under `/q/` it is an unknown query, not run.
    let mismatched = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/place_order",
        json!({}),
        json!({}),
    );
    assert_eq!(mismatched.status, 404);
    assert_eq!(mismatched.body["error"]["code"], "unknown_query");
    assert!(db.calls.is_empty());
}

/// A database fault (connection lost, deadlock, shard down) surfaces as a retryable
/// 503 `database_error`, distinct from a 4xx caller error.
#[test]
fn db_fault_maps_to_503() {
    let c = compile(SCHEMA);
    let mut db = MockDb::failing("connection reset by peer");
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &mut db,
        &mut ids,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
    );
    assert_eq!(resp.status, 503);
    assert_eq!(resp.body["error"]["code"], "database_error");
    assert_eq!(resp.body["error"]["message"], "connection reset by peer");
}

/// A malformed route path is a 404 `not_found`; a non-POST method is 405.
#[test]
fn bad_route_and_method() {
    let c = compile(SCHEMA);
    let mut db = MockDb::new(vec![]);
    let mut ids = SeqIdGen::default();

    for path in ["/", "/q", "/q/", "/x/order_by_id", "/q/a/b"] {
        let r = dispatch(&c, &mut db, &mut ids, "POST", path, json!({}), json!({}));
        assert_eq!(r.status, 404, "path {path}");
        assert_eq!(r.body["error"]["code"], "not_found", "path {path}");
    }

    let get = dispatch(
        &c,
        &mut db,
        &mut ids,
        "GET",
        "/q/order_by_id",
        json!({}),
        json!({}),
    );
    assert_eq!(get.status, 405);
    assert_eq!(get.body["error"]["code"], "method_not_allowed");
}
