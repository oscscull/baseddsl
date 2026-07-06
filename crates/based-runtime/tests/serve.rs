//! Wire-dispatch tests: a decoded HTTP request (method, path, JSON args, `$ctx`) →
//! a `WireResponse` (status + JSON body). The whole route → plan → run → response
//! path runs against a `MockDb`, no network and no database.
//!
//! Headline assertions: (1) the route prefix selects query vs mutation and 404s on a
//! bad/mismatched route; (2) success returns 200 + the shaped response; (3) every
//! `PlanError` maps to its HTTP status + `{ error: { code, message } }`; (4) only POST
//! is accepted (no GET query-string surface, calling.md); (5) a mutation idempotency
//! key dedupes a retry, and (6) reusing one key for a *different* request is a 422 (D25).

use based_ast::FileId;
use based_parser::parse_file;
use based_sema::check;
use serde_json::json;

use based_runtime::{dispatch, Compiled, MemStore, MockDb, NoStore, Row, SeqIdGen};

fn compile(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error)
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
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
        &NoStore,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
        None,
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
        &NoStore,
        "POST",
        "/q/orders_in_org",
        json!({ "org": "org-1" }),
        json!({}),
        None,
    );
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_array());
    assert_eq!(resp.body.as_array().unwrap().len(), 2);
}

/// A mutation route runs the write path and returns the created row in its declared
/// shape (D12), read back inside the transaction.
#[test]
fn mutation_route_returns_the_created_rows_declared_shape() {
    let c = compile(SCHEMA);
    // The re-select of `OrderCard { status, total }` after the INSERT.
    let mut db = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let mut ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &mut db,
        &mut ids,
        &NoStore,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        None,
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "open", "total": 7 }));
    // INSERT then the shaped re-select, between one begin/commit.
    assert_eq!(db.tx, vec!["begin", "commit"]);
    assert_eq!(db.calls.len(), 2);
    assert!(db.calls[0].0.contains("INSERT INTO `order`"));
    assert!(db.calls[1].0.starts_with("SELECT"), "{}", db.calls[1].0);
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
        &NoStore,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-9" }),
        None,
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
        &NoStore,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({}),
        None,
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
        &NoStore,
        "POST",
        "/q/order_by_id",
        json!({}),
        json!({}),
        None,
    );
    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"]["code"], "missing_arg");

    let bad = dispatch(
        &c,
        &mut db,
        &mut ids,
        &NoStore,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": "not-an-int" }),
        json!({}),
        None,
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
        &NoStore,
        "POST",
        "/q/nope",
        json!({}),
        json!({}),
        None,
    );
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.body["error"]["code"], "unknown_query");

    // `place_order` is a mutation; under `/q/` it is an unknown query, not run.
    let mismatched = dispatch(
        &c,
        &mut db,
        &mut ids,
        &NoStore,
        "POST",
        "/q/place_order",
        json!({}),
        json!({}),
        None,
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
        &NoStore,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
        None,
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
        let r = dispatch(
            &c,
            &mut db,
            &mut ids,
            &NoStore,
            "POST",
            path,
            json!({}),
            json!({}),
            None,
        );
        assert_eq!(r.status, 404, "path {path}");
        assert_eq!(r.body["error"]["code"], "not_found", "path {path}");
    }

    let get = dispatch(
        &c,
        &mut db,
        &mut ids,
        &NoStore,
        "GET",
        "/q/order_by_id",
        json!({}),
        json!({}),
        None,
    );
    assert_eq!(get.status, 405);
    assert_eq!(get.body["error"]["code"], "method_not_allowed");
}

/// An idempotency key makes a mutation retry a no-op: the write body runs once, and the
/// retry replays the first response with no second INSERT (D25). The retry uses a *fresh*
/// `SeqIdGen`, so a naive re-run would mint a different id — the replay proves it didn't.
#[test]
fn idempotency_key_dedupes_a_mutation_retry() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // First attempt: the INSERT + the shaped re-select run, response recorded.
    let mut db1 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let mut ids1 = SeqIdGen::default();
    let first = dispatch(
        &c,
        &mut db1,
        &mut ids1,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-42".to_string()),
    );
    assert_eq!(first.status, 200);
    assert_eq!(first.body, json!({ "status": "open", "total": 7 }));
    assert_eq!(db1.tx, vec!["begin", "commit"]);
    assert_eq!(db1.calls.len(), 2);

    // Retry with the same key on a fresh connection: replayed, no SQL, no transaction.
    let mut db2 = MockDb::new(vec![vec![row(json!({ "status": "SHOULD-NOT-BE-READ" }))]]);
    let mut ids2 = SeqIdGen::default();
    let retry = dispatch(
        &c,
        &mut db2,
        &mut ids2,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-42".to_string()),
    );
    assert_eq!(retry.status, 200);
    // The first attempt's response, not the second mock's canned row.
    assert_eq!(retry.body, json!({ "status": "open", "total": 7 }));
    assert!(db2.calls.is_empty(), "retry must run no SQL");
    assert!(db2.tx.is_empty(), "retry must open no transaction");
}

/// A keyed mutation whose first attempt *fails* (DB fault) does not poison the key: a
/// retry re-runs the write (the failure was rolled back, so nothing committed to replay).
#[test]
fn failed_keyed_mutation_is_retryable() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // First attempt faults mid-write → 503, key abandoned.
    let mut db1 = MockDb::failing("deadlock");
    let mut ids1 = SeqIdGen::default();
    let first = dispatch(
        &c,
        &mut db1,
        &mut ids1,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-7".to_string()),
    );
    assert_eq!(first.status, 503);

    // Retry with the same key succeeds — it was not blocked as a duplicate.
    let mut db2 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let mut ids2 = SeqIdGen::default();
    let retry = dispatch(
        &c,
        &mut db2,
        &mut ids2,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-7".to_string()),
    );
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body, json!({ "status": "open", "total": 7 }));
}

/// A malformed keyed request (bad arg) is a clean 400 that consumes no idempotency slot:
/// the same key then works once the request is fixed (planning precedes the store).
#[test]
fn bad_request_does_not_consume_the_key() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // A mistyped `total` → 400 before any store interaction.
    let mut db1 = MockDb::new(vec![]);
    let mut ids1 = SeqIdGen::default();
    let bad = dispatch(
        &c,
        &mut db1,
        &mut ids1,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": "nope" }),
        json!({}),
        Some("req-9".to_string()),
    );
    assert_eq!(bad.status, 400);

    // The corrected request with the same key runs — the key was never claimed.
    let mut db2 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let mut ids2 = SeqIdGen::default();
    let ok = dispatch(
        &c,
        &mut db2,
        &mut ids2,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-9".to_string()),
    );
    assert_eq!(ok.status, 200);
    assert_eq!(db2.calls.len(), 2);
}

/// Reusing one idempotency key for a *different* request is rejected with a 422, not
/// silently answered with the first request's response (D25 fingerprint check). The first
/// attempt commits under `req-x`; a second request under the *same* key but a different
/// `total` must not run a write and must not replay the first row.
#[test]
fn reused_key_with_different_args_is_a_422() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // First request under the key: runs and records its response.
    let mut db1 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let mut ids1 = SeqIdGen::default();
    let first = dispatch(
        &c,
        &mut db1,
        &mut ids1,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-x".to_string()),
    );
    assert_eq!(first.status, 200);

    // Same key, *different* payload (`total` 999) → 422, no write, no replay.
    let mut db2 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 999 }))]]);
    let mut ids2 = SeqIdGen::default();
    let reuse = dispatch(
        &c,
        &mut db2,
        &mut ids2,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 999 }),
        json!({}),
        Some("req-x".to_string()),
    );
    assert_eq!(reuse.status, 422);
    assert_eq!(reuse.body["error"]["code"], "idempotency_key_reuse");
    assert!(
        db2.calls.is_empty(),
        "a key-reuse rejection must run no SQL"
    );
    assert!(
        db2.tx.is_empty(),
        "a key-reuse rejection must open no transaction"
    );

    // The genuine retry (same key *and* payload) still replays the first response — the
    // mismatch above didn't disturb the recorded entry.
    let mut db3 = MockDb::new(vec![]);
    let mut ids3 = SeqIdGen::default();
    let retry = dispatch(
        &c,
        &mut db3,
        &mut ids3,
        &store,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-x".to_string()),
    );
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body, json!({ "status": "open", "total": 7 }));
    assert!(
        db3.calls.is_empty(),
        "the genuine retry replays, runs no SQL"
    );
}
