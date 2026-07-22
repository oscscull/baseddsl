//! Streaming read tests: the `-> stream` dispatch surface (`run_query_stream` /
//! `dispatch_stream`) yields shaped rows one at a time off the driver's row stream.
//! Pre-body failures (validation, unknown route) surface before the first row with
//! their ordinary statuses; a mid-stream database failure is the stream's last item;
//! dropping the stream mid-pass returns its connection to the pool healthy. Mock-backed
//! tests pin the semantics; the SQLite tests prove them against a real engine.

use based_ast::FileId;
use based_parser::parse_file;
use based_sema::check;
use futures_util::StreamExt;
use serde_json::json;

use based_runtime::plan::PlanError;
use based_runtime::{dispatch_stream, run_query_stream, Compiled, Engine, MockDb, Request, Row};

const SCHEMA: &str = r#"
    Org { id: Id, name: text }
    @sort(total desc)
    Order { id: Id, org: Org, status: text, total: int }
    shape OrderCard from Order { status, total }

    query export_orders(org) -> stream OrderCard;
    query order_by_id(id) -> OrderCard;
    mutation place_order(org: Id, status, total: int) -> OrderCard {
        create Order { org = $org, status = $status, total = $total };
    }
"#;

fn compile(src: &str) -> Compiled {
    let sf = parse_file(src, FileId(0)).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    let (schema, diags) = check(&sf.decls);
    let errs: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260")
        .map(|d| d.code)
        .collect();
    assert!(errs.is_empty(), "unexpected sema errors: {errs:?}");
    Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
}

fn row(pairs: serde_json::Value) -> Row {
    pairs.as_object().cloned().unwrap()
}

#[tokio::test]
async fn stream_yields_shaped_rows_in_order() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 9 })),
        row(json!({ "status": "open", "total": 3 })),
    ]]);

    let req = Request::new("export_orders", json!({ "org": "org-1" }), json!({}));
    let stream = run_query_stream(&c, Box::new(db), &req).expect("plans clean");
    let items: Vec<_> = stream.collect().await;

    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].as_ref().unwrap(),
        &json!({ "status": "paid", "total": 9 })
    );
    assert_eq!(
        items[1].as_ref().unwrap(),
        &json!({ "status": "open", "total": 3 })
    );
}

#[tokio::test]
async fn boundary_failure_surfaces_before_the_first_row() {
    // A missing required arg is a plan error: the stream never starts.
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![]);
    let req = Request::new("export_orders", json!({}), json!({}));
    let err = run_query_stream(&c, Box::new(db.clone()), &req).err();
    assert_eq!(err, Some(PlanError::MissingArg("org".into())));
    // Nothing reached the database.
    assert!(db.calls().is_empty());
}

#[tokio::test]
async fn mid_stream_failure_is_the_last_item() {
    let c = compile(SCHEMA);
    let db = MockDb::failing_mid_stream(
        vec![row(json!({ "status": "paid", "total": 9 }))],
        "connection lost",
    );

    let req = Request::new("export_orders", json!({ "org": "org-1" }), json!({}));
    let stream = run_query_stream(&c, Box::new(db), &req).expect("plans clean");
    let items: Vec<_> = stream.collect().await;

    assert_eq!(items.len(), 2);
    assert!(items[0].is_ok());
    let e = items[1].as_ref().unwrap_err();
    assert_eq!(e.code(), "database_error");
    assert_eq!(e.message, "connection lost");
}

#[tokio::test]
async fn dispatch_stream_rejects_pre_body_failures_with_wire_statuses() {
    let c = compile(SCHEMA);
    let db = MockDb::new(vec![]);

    // Unknown query → the ordinary 404.
    let err = dispatch_stream(&c, &db, "", "POST", "/q/nope", json!({}), json!({}))
        .await
        .err()
        .expect("unknown query is rejected");
    assert_eq!(err.status, 404);
    assert_eq!(err.body["error"]["code"], "unknown_query");

    // Bad args → 400 with the same code the buffered wire uses.
    let err = dispatch_stream(
        &c,
        &db,
        "",
        "POST",
        "/q/export_orders",
        json!({}),
        json!({}),
    )
    .await
    .err()
    .expect("missing arg is rejected");
    assert_eq!(err.status, 400);
    assert_eq!(err.body["error"]["code"], "missing_arg");

    // A declared non-stream query through the streaming surface is an internal misuse.
    let err = dispatch_stream(
        &c,
        &db,
        "",
        "POST",
        "/q/order_by_id",
        json!({ "id": "o-1" }),
        json!({}),
    )
    .await
    .err()
    .expect("non-stream query is rejected");
    assert_eq!(err.status, 500);
    assert_eq!(err.body["error"]["code"], "internal");

    // Mutations never stream.
    let err = dispatch_stream(&c, &db, "", "POST", "/m/place_order", json!({}), json!({}))
        .await
        .err()
        .expect("mutation route is rejected");
    assert_eq!(err.status, 500);
}

#[tokio::test]
async fn engine_call_stream_yields_rows_in_process() {
    let db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 9 })),
        row(json!({ "status": "open", "total": 3 })),
    ]]);
    let engine = Engine::new(compile(SCHEMA), db, based_runtime::SeqIdGen::default());

    let stream = engine
        .call_stream("/q/export_orders", json!({ "org": "org-1" }), json!({}))
        .await
        .expect("stream starts");
    let items: Vec<_> = stream.collect().await;
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.is_ok()));
}

// ---- against a real engine (in-memory SQLite) ------------------------------

#[cfg(feature = "sqlite")]
mod sqlite {
    use super::*;
    use based_codegen::{sql, Dialect};
    use based_runtime::SqliteBackend;

    /// Seed an in-memory SQLite with the generated DDL + three orders.
    async fn backend(c: &Compiled) -> SqliteBackend {
        let b = SqliteBackend::in_memory().expect("open in-memory sqlite");
        let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
        b.execute_batch(&ddl)
            .await
            .unwrap_or_else(|e| panic!("generated DDL failed: {e:?}\n{ddl}"));
        b.execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme');
            INSERT INTO `order` (`id`, `org_id`, `status`, `total`) VALUES
                ('o-1', 'org-1', 'open', 10),
                ('o-2', 'org-1', 'paid', 30),
                ('o-3', 'org-1', 'paid', 20);
            "#,
        )
        .await
        .expect("seed fixtures");
        b
    }

    #[tokio::test]
    async fn rows_arrive_in_sort_order_off_a_real_engine() {
        let c = compile(SCHEMA);
        let b = backend(&c).await;

        let stream = dispatch_stream(
            &c,
            &b,
            "",
            "POST",
            "/q/export_orders",
            json!({ "org": "org-1" }),
            json!({}),
        )
        .await
        .expect("stream starts");
        let rows: Vec<_> = stream
            .map(|r| r.expect("row decodes"))
            .collect::<Vec<_>>()
            .await;

        // The model `@sort(total desc)` orders the pass.
        assert_eq!(
            rows,
            vec![
                json!({ "status": "paid", "total": 30 }),
                json!({ "status": "paid", "total": 20 }),
                json!({ "status": "open", "total": 10 }),
            ]
        );
    }

    #[tokio::test]
    async fn dropping_the_stream_returns_the_connection_healthy() {
        let c = compile(SCHEMA);
        let b = backend(&c).await;

        // Take one row, then drop the stream mid-pass (the caller cancelled).
        let mut stream = dispatch_stream(
            &c,
            &b,
            "",
            "POST",
            "/q/export_orders",
            json!({ "org": "org-1" }),
            json!({}),
        )
        .await
        .expect("stream starts");
        let first = stream.next().await.expect("has a first row");
        assert!(first.is_ok());
        drop(stream);

        // The backend's pool holds exactly one connection: the next read only works if
        // the cancelled pass returned it healthy.
        let full: Vec<_> = dispatch_stream(
            &c,
            &b,
            "",
            "POST",
            "/q/export_orders",
            json!({ "org": "org-1" }),
            json!({}),
        )
        .await
        .expect("stream starts on the recycled connection")
        .collect()
        .await;
        assert_eq!(full.len(), 3);
        assert!(full.iter().all(|r| r.is_ok()));
    }
}
