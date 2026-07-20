//! Wire-dispatch tests: a decoded HTTP request (method, path, JSON args, `$ctx`) →
//! a `WireResponse` (status + JSON body). The whole route → plan → run → response
//! path runs against a `MockDb`, no network and no database.
//!
//! Headline assertions: (1) the route prefix selects query vs mutation and 404s on a
//! bad/mismatched route; (2) success returns 200 + the shaped response; (3) every
//! `PlanError` maps to its HTTP status + `{ error: { code, message } }`; (4) only POST
//! is accepted (no GET query-string surface); (5) a mutation idempotency
//! key dedupes a retry, and (6) reusing one key for a *different* request is a 422.

use based_ast::FileId;
use based_parser::parse_file;
use based_sema::check;
use serde_json::json;

use based_runtime::{dispatch, Compiled, Guards, MemStore, MockDb, NoStore, Row, SeqIdGen};

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
#[tokio::test]
async fn query_route_returns_shaped_response() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "paid", "total": 42 }));
    // The arg bound positionally into the fetched statement.
    assert_eq!(
        db.calls()[0].1,
        vec![based_runtime::SqlValue::Uuid("o-1".into())]
    );
}

/// A `list` route returns an array (envelope Many).
#[tokio::test]
async fn list_route_returns_array() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 1 })),
        row(json!({ "status": "open", "total": 2 })),
    ]]);
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/orders_in_org",
        json!({ "org": "org-1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_array());
    assert_eq!(resp.body.as_array().unwrap().len(), 2);
}

/// A mutation route runs the write path and returns the created row in its declared
/// shape, read back inside the transaction.
#[tokio::test]
async fn mutation_route_returns_the_created_rows_declared_shape() {
    let c = compile(SCHEMA);
    // The re-select of `OrderCard { status, total }` after the INSERT.
    let db = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "open", "total": 7 }));
    // INSERT then the shaped re-select, between one begin/commit.
    assert_eq!(db.tx_log(), vec!["begin", "commit"]);
    let calls = db.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].0.contains("INSERT INTO `order`"));
    assert!(calls[1].0.starts_with("SELECT"), "{}", calls[1].0);
}

/// A mutation whose `where` matched no row (a wrong or out-of-scope id) is a 404
/// `not_found` — never a `200` with a null body the typed client cannot decode.
#[tokio::test]
async fn zero_row_mutation_maps_to_404_not_found() {
    let c = compile(
        r#"
        Order { status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation set_status(id: Id, status: text) -> OrderCard {
            update Order where (id = $id) { status = $status };
        }
        "#,
    );
    // The UPDATE matches nothing, so the declared-shape re-select reads back no row.
    let db = MockDb::new(vec![vec![]]);
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/m/set_status",
        json!({ "id": "no-such", "status": "shipped" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.body["error"]["code"], "not_found");
    // The miss rolled the transaction back.
    assert_eq!(db.tx_log(), vec!["begin", "rollback"]);
}

/// `$ctx` arrives as the server-supplied context (not the body); a required one that
/// is missing is a 400 `missing_ctx`.
#[tokio::test]
async fn ctx_supplied_out_of_band_and_required() {
    let c = compile(SCHEMA);
    let ids = SeqIdGen::default();

    // Provided → 200.
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 1 }))]]);
    let ok = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({ "org": "org-9" }),
        None,
    )
    .await;
    assert_eq!(ok.status, 200);
    assert_eq!(
        db.calls()[0].1,
        vec![based_runtime::SqlValue::Uuid("org-9".into())]
    );

    // Missing → 400 missing_ctx.
    let db2 = MockDb::new(vec![]);
    let miss = dispatch(
        &c,
        &db2,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/my_org_orders",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(miss.status, 400);
    assert_eq!(miss.body["error"]["code"], "missing_ctx");
}

/// A missing required arg is a 400 `missing_arg`; a mistyped arg is a 400 `bad_arg`.
#[tokio::test]
async fn arg_validation_maps_to_400() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![]);
    let ids = SeqIdGen::default();

    let missing = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/order_by_id",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"]["code"], "missing_arg");

    let bad = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": "not-an-int" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(bad.status, 400);
    assert_eq!(bad.body["error"]["code"], "bad_arg");
    // No SQL ran — validation is at the boundary.
    assert!(db.calls().is_empty());
}

/// An unknown callable is a 404; the route prefix is authoritative (a mutation name
/// under `/q/` is an unknown *query*, never a cross-dispatch).
#[tokio::test]
async fn unknown_and_mismatched_routes_404() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![]);
    let ids = SeqIdGen::default();

    let unknown = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/nope",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.body["error"]["code"], "unknown_query");

    // `place_order` is a mutation; under `/q/` it is an unknown query, not run.
    let mismatched = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/place_order",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(mismatched.status, 404);
    assert_eq!(mismatched.body["error"]["code"], "unknown_query");
    assert!(db.calls().is_empty());
}

/// A database fault (connection lost, deadlock, shard down) surfaces as a retryable
/// 503 `database_error`, distinct from a 4xx caller error.
#[tokio::test]
async fn db_fault_maps_to_503() {
    let c = compile(SCHEMA);
    let db = MockDb::failing("connection reset by peer");
    let ids = SeqIdGen::default();
    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 503);
    assert_eq!(resp.body["error"]["code"], "database_error");
    assert_eq!(resp.body["error"]["message"], "connection reset by peer");
}

/// A malformed route path is a 404 `not_found`; a non-POST method is 405.
#[tokio::test]
async fn bad_route_and_method() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![]);
    let ids = SeqIdGen::default();

    for path in ["/", "/q", "/q/", "/x/order_by_id", "/q/a/b"] {
        let r = dispatch(
            &c,
            &db,
            "",
            &ids,
            &NoStore,
            &Guards::new(),
            None,
            "POST",
            path,
            json!({}),
            json!({}),
            None,
        )
        .await;
        assert_eq!(r.status, 404, "path {path}");
        assert_eq!(r.body["error"]["code"], "not_found", "path {path}");
    }

    let get = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "GET",
        "/q/order_by_id",
        json!({}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(get.status, 405);
    assert_eq!(get.body["error"]["code"], "method_not_allowed");
}

/// An idempotency key makes a mutation retry a no-op: the write body runs once, and the
/// retry replays the first response with no second INSERT. The retry uses a *fresh*
/// `SeqIdGen`, so a naive re-run would mint a different id — the replay proves it didn't.
#[tokio::test]
async fn idempotency_key_dedupes_a_mutation_retry() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // First attempt: the INSERT + the shaped re-select run, response recorded.
    let db1 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let ids1 = SeqIdGen::default();
    let first = dispatch(
        &c,
        &db1,
        "",
        &ids1,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-42".to_string()),
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(first.body, json!({ "status": "open", "total": 7 }));
    assert_eq!(db1.tx_log(), vec!["begin", "commit"]);
    assert_eq!(db1.calls().len(), 2);

    // Retry with the same key on a fresh connection: replayed, no SQL, no transaction.
    let db2 = MockDb::new(vec![vec![row(json!({ "status": "SHOULD-NOT-BE-READ" }))]]);
    let ids2 = SeqIdGen::default();
    let retry = dispatch(
        &c,
        &db2,
        "",
        &ids2,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-42".to_string()),
    )
    .await;
    assert_eq!(retry.status, 200);
    // The first attempt's response, not the second mock's canned row.
    assert_eq!(retry.body, json!({ "status": "open", "total": 7 }));
    assert!(db2.calls().is_empty(), "retry must run no SQL");
    assert!(db2.tx_log().is_empty(), "retry must open no transaction");
}

/// A keyed mutation whose first attempt *fails* (DB fault) does not poison the key: a
/// retry re-runs the write (the failure was rolled back, so nothing committed to replay).
#[tokio::test]
async fn failed_keyed_mutation_is_retryable() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // First attempt faults mid-write → 503, key abandoned.
    let db1 = MockDb::failing("connection lost");
    let ids1 = SeqIdGen::default();
    let first = dispatch(
        &c,
        &db1,
        "",
        &ids1,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-7".to_string()),
    )
    .await;
    assert_eq!(first.status, 503);

    // Retry with the same key succeeds — it was not blocked as a duplicate.
    let db2 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let ids2 = SeqIdGen::default();
    let retry = dispatch(
        &c,
        &db2,
        "",
        &ids2,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-7".to_string()),
    )
    .await;
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body, json!({ "status": "open", "total": 7 }));
}

/// A malformed keyed request (bad arg) is a clean 400 that consumes no idempotency slot:
/// the same key then works once the request is fixed (planning precedes the store).
#[tokio::test]
async fn bad_request_does_not_consume_the_key() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // A mistyped `total` → 400 before any store interaction.
    let db1 = MockDb::new(vec![]);
    let ids1 = SeqIdGen::default();
    let bad = dispatch(
        &c,
        &db1,
        "",
        &ids1,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": "nope" }),
        json!({}),
        Some("req-9".to_string()),
    )
    .await;
    assert_eq!(bad.status, 400);

    // The corrected request with the same key runs — the key was never claimed.
    let db2 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let ids2 = SeqIdGen::default();
    let ok = dispatch(
        &c,
        &db2,
        "",
        &ids2,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-9".to_string()),
    )
    .await;
    assert_eq!(ok.status, 200);
    assert_eq!(db2.calls().len(), 2);
}

/// Reusing one idempotency key for a *different* request is rejected with a 422, not
/// silently answered with the first request's response. The first
/// attempt commits under `req-x`; a second request under the *same* key but a different
/// `total` must not run a write and must not replay the first row.
#[tokio::test]
async fn reused_key_with_different_args_is_a_422() {
    let c = compile(SCHEMA);
    let store = MemStore::new();

    // First request under the key: runs and records its response.
    let db1 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let ids1 = SeqIdGen::default();
    let first = dispatch(
        &c,
        &db1,
        "",
        &ids1,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-x".to_string()),
    )
    .await;
    assert_eq!(first.status, 200);

    // Same key, *different* payload (`total` 999) → 422, no write, no replay.
    let db2 = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 999 }))]]);
    let ids2 = SeqIdGen::default();
    let reuse = dispatch(
        &c,
        &db2,
        "",
        &ids2,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 999 }),
        json!({}),
        Some("req-x".to_string()),
    )
    .await;
    assert_eq!(reuse.status, 422);
    assert_eq!(reuse.body["error"]["code"], "idempotency_key_reuse");
    assert!(
        db2.calls().is_empty(),
        "a key-reuse rejection must run no SQL"
    );
    assert!(
        db2.tx_log().is_empty(),
        "a key-reuse rejection must open no transaction"
    );

    // The genuine retry (same key *and* payload) still replays the first response — the
    // mismatch above didn't disturb the recorded entry.
    let db3 = MockDb::new(vec![]);
    let ids3 = SeqIdGen::default();
    let retry = dispatch(
        &c,
        &db3,
        "",
        &ids3,
        &store,
        &Guards::new(),
        None,
        "POST",
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
        Some("req-x".to_string()),
    )
    .await;
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body, json!({ "status": "open", "total": 7 }));
    assert!(
        db3.calls().is_empty(),
        "the genuine retry replays, runs no SQL"
    );
}

// ---------- guards (auth.md Handle 3) ---------------------------------------

const GUARDED_SCHEMA: &str = r#"
    Order { status: text, total: int }
    shape OrderCard from Order { status, total }
    mutation close_order(id) -> OrderCard guard caller_can_close {
        update Order where (id = $id) { status = "closed" };
    }
"#;

/// The declared guard runs before the write body and receives the callable's name,
/// its args, and the server-derived `$ctx`; an allow lets the mutation proceed.
#[tokio::test]
async fn declared_guard_runs_and_allows() {
    use based_runtime::GuardVerdict;
    use std::sync::{Arc, Mutex};

    let c = compile(GUARDED_SCHEMA);
    let db = MockDb::new(vec![vec![row(json!({ "status": "closed", "total": 9 }))]]);
    let seen: Arc<Mutex<Option<based_runtime::GuardRequest>>> = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&seen);
    let guards = Guards::new().register("caller_can_close", move |req| {
        *observed.lock().unwrap() = Some(req);
        async { GuardVerdict::Allow }
    });
    let ids = SeqIdGen::default();

    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &guards,
        None,
        "POST",
        "/m/close_order",
        json!({ "id": "o-1" }),
        json!({ "role": "agent" }),
        None,
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(db.tx_log(), vec!["begin", "commit"]);

    let req = seen.lock().unwrap().clone().expect("the guard ran");
    assert_eq!(req.callable, "close_order");
    assert_eq!(req.args, json!({ "id": "o-1" }));
    assert_eq!(req.ctx, json!({ "role": "agent" }));
}

/// A denial is a `403` with the stable code and the guard's reason, and the write
/// never runs — no SQL, no transaction.
#[tokio::test]
async fn guard_denial_is_403_and_runs_no_sql() {
    use based_runtime::GuardVerdict;

    let c = compile(GUARDED_SCHEMA);
    let db = MockDb::new(vec![]);
    let guards = Guards::new().register("caller_can_close", |_req| async {
        GuardVerdict::deny("only agents may close orders")
    });
    let ids = SeqIdGen::default();

    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &guards,
        None,
        "POST",
        "/m/close_order",
        json!({ "id": "o-1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 403);
    assert_eq!(resp.body["error"]["code"], "guard_denied");
    assert_eq!(
        resp.body["error"]["message"],
        "only agents may close orders"
    );
    assert!(db.calls().is_empty(), "a denied mutation runs no SQL");
    assert!(
        db.tx_log().is_empty(),
        "a denied mutation opens no transaction"
    );
}

/// The request-time backstop for a raw dispatch with an unregistered declared guard:
/// a loud `500`, never a silent pass. (Engine build / listener startup refuse the
/// pairing before any request exists.)
#[tokio::test]
async fn unregistered_declared_guard_is_a_loud_500() {
    let c = compile(GUARDED_SCHEMA);
    let db = MockDb::new(vec![]);
    let ids = SeqIdGen::default();

    let resp = dispatch(
        &c,
        &db,
        "",
        &ids,
        &NoStore,
        &Guards::new(),
        None,
        "POST",
        "/m/close_order",
        json!({ "id": "o-1" }),
        json!({}),
        None,
    )
    .await;
    assert_eq!(resp.status, 500);
    assert_eq!(resp.body["error"]["code"], "guard_unregistered");
    assert!(resp.body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("caller_can_close"));
    assert!(
        db.calls().is_empty(),
        "an unenforceable mutation runs no SQL"
    );
}

/// A denied keyed mutation never claims its idempotency key: once the caller is
/// allowed, the same key runs fresh instead of hitting a stranded claim or a replay.
#[tokio::test]
async fn guard_denial_never_claims_the_idempotency_key() {
    use based_runtime::GuardVerdict;

    let c = compile(GUARDED_SCHEMA);
    let store = MemStore::default();
    let ids = SeqIdGen::default();

    let deny = Guards::new().register("caller_can_close", |_req| async {
        GuardVerdict::deny("not yet")
    });
    let denied = dispatch(
        &c,
        &MockDb::new(vec![]),
        "",
        &ids,
        &store,
        &deny,
        None,
        "POST",
        "/m/close_order",
        json!({ "id": "o-1" }),
        json!({}),
        Some("key-guarded".to_string()),
    )
    .await;
    assert_eq!(denied.status, 403);

    // The same key, now allowed: a fresh run (200 from the database), not a conflict
    // and not a replay of anything.
    let db = MockDb::new(vec![vec![row(json!({ "status": "closed", "total": 9 }))]]);
    let allow = Guards::new().register("caller_can_close", |_req| async { GuardVerdict::Allow });
    let allowed = dispatch(
        &c,
        &db,
        "",
        &ids,
        &store,
        &allow,
        None,
        "POST",
        "/m/close_order",
        json!({ "id": "o-1" }),
        json!({}),
        Some("key-guarded".to_string()),
    )
    .await;
    assert_eq!(allowed.status, 200, "{:?}", allowed.body);
    assert_eq!(
        db.tx_log(),
        vec!["begin", "commit"],
        "the allowed run is fresh"
    );
}
