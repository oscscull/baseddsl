//! Worked embed example (Tier 1): the **generated typed client** running in-process
//! over [`Engine`], with **no socket** — the library twin of `based serve`.
//!
//! This is the end-to-end proof of the in-process door. `mod client` below is the
//! *verbatim* output of `based gen client` for `SCHEMA`, generated **with the embedded
//! bridge** (`ClientOptions::embedded`) — committed so the test exercises the real
//! generated surface, not a hand-written stand-in. The payoff: an embedder writes **zero**
//! bridge code. The `Transport` impl over `Engine` is now *emitted* by codegen (the
//! `Embedded` transport + the `embedded(&engine)` constructor at the bottom of the
//! module), so `client::embedded(&engine)` hands back a ready client — no more copying the
//! ~20-line `InProcess` bridge into every consumer.
//!
//! The visible payoff: `client.order_by_id(...)` returns a typed `Option<OrderCard>`
//! decoded from the engine's shaped JSON — the same typed call an HTTP client would make,
//! minus the loopback socket + HTTP framing (the win is dropping the socket, not the
//! JSON).

use based_ast::FileId;
use based_runtime::{Compiled, Engine, MockDb, Row, SeqIdGen};
use serde_json::json;

/// The **verbatim** `based gen client` output for `SCHEMA` (target: rust, embedded bridge
/// on), committed as `tests/support/embedded_client.rs` so the tests exercise the real
/// generated surface, not a hand-written stand-in. It is re-verified against the live
/// generator by [`generated_client_is_current`], so it can never silently drift; regenerate
/// with `based gen client --embedded` if `SCHEMA` changes.
#[allow(dead_code)]
mod client {
    include!("support/embedded_client.rs");
}

/// The schema `mod client` was generated from — loaded into the engine so routes and
/// wire shapes line up on both sides. Shared with `tests/streaming_client.rs` (the
/// HTTP twin over the same generated client), so the two suites can never drift.
const SCHEMA: &str = include_str!("support/embedded_client_schema.bsl");

fn compiled() -> Compiled {
    let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = based_sema::check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean"
    );
    Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
}

fn row(v: serde_json::Value) -> Row {
    v.as_object().cloned().unwrap()
}

/// The regen gate: the committed `tests/support/embedded_client.rs` is exactly what
/// `based gen client --embedded` emits for `SCHEMA` today. If codegen changes the client
/// surface, this fails until the committed mirror is regenerated — so `mod client` above
/// can never drift into a stale hand-copy.
#[test]
fn generated_client_is_current() {
    use based_codegen::client::{client_with, ClientOptions, ClientTarget};

    let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = based_sema::check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean"
    );
    let generated = client_with(
        &schema,
        &sf.decls,
        ClientTarget::Rust,
        ClientOptions { embedded: true },
    );
    assert_eq!(
        generated,
        include_str!("support/embedded_client.rs"),
        "tests/support/embedded_client.rs is stale — regenerate with `based gen client --embedded`"
    );
}

/// A typed `get` round-trips: the engine's shaped `200` decodes into `Option<OrderCard>`.
/// The client comes straight from the generated `client::embedded(&engine)` — no bridge.
#[tokio::test]
async fn typed_get_round_trips_in_process() {
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let got = api
        .order_by_id(
            client::OrderByIdInput {
                id: client::Id::from_raw("o-1"),
            },
            (),
        )
        .await
        .expect("call ok");
    let card = got.expect("a row");
    assert_eq!(card.status, "paid");
    assert_eq!(card.total, 42);
}

/// A `get` that matches no row decodes to `None` (envelope `One` → JSON `null`).
#[tokio::test]
async fn typed_get_missing_is_none() {
    let db = MockDb::new(vec![vec![]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let got = api
        .order_by_id(
            client::OrderByIdInput {
                id: client::Id::from_raw("missing"),
            },
            (),
        )
        .await
        .expect("call ok");
    assert!(got.is_none());
}

/// A typed `list` decodes into `Vec<OrderCard>`.
#[tokio::test]
async fn typed_list_round_trips_in_process() {
    let db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 9 })),
        row(json!({ "status": "open", "total": 3 })),
    ]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let rows = api
        .orders_in_org(
            client::OrdersInOrgInput {
                org: client::Id::from_raw("org-1"),
            },
            (),
        )
        .await
        .expect("call ok");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].total, 9);
    assert_eq!(rows[1].status, "open");
}

/// `$ctx` is a **typed** argument on the generated method: `my_org_orders` takes a
/// `MyOrgOrdersCtx { org }`, supplied straight in — no header dance and no
/// untyped side-channel bag. With the required context the `$ctx`-scoped query runs; an
/// empty context (the embedded bridge maps `&()` → `{}`) makes the engine's boundary `400`
/// surface as the client's `ClientError` (the same non-200 an HTTP client sees).
#[tokio::test]
async fn ctx_supplied_in_process_and_required() {
    // With MyOrgOrdersCtx.org present → the query runs and decodes.
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 1 }))]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);
    let rows = api
        .my_org_orders(
            client::MyOrgOrdersInput,
            client::MyOrgOrdersCtx {
                org: client::Id::from_raw("org-9"),
            },
        )
        .await
        .expect("call ok");
    assert_eq!(rows.len(), 1);

    // A route that requires `$ctx.org` but is reached with an empty context bag (the
    // untyped raw path here, mirroring a missing header) → a boundary 400.
    let err = engine.call("/q/my_org_orders", json!({}), json!({})).await;
    assert_eq!(err.status, 400);
    assert!(
        err.body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("ctx")),
        "message names the missing ctx: {}",
        err.body
    );
}

/// The write path runs in-process and returns the created row in its **declared shape**:
/// after the INSERT the engine re-selects the created `Order` as an `OrderCard`,
/// still inside the transaction, and *that* is the `200` body — so the typed
/// `place_order` method decodes clean into an `OrderCard`, exactly like a `get`.
#[tokio::test]
async fn mutation_response_is_the_created_rows_declared_shape() {
    // Two writes below (raw + typed), each answered by a post-write re-select of the
    // shaped row.
    let engine = Engine::new(
        compiled(),
        MockDb::new(vec![
            vec![row(json!({ "status": "open", "total": 7 }))],
            vec![row(json!({ "status": "open", "total": 7 }))],
        ]),
        SeqIdGen::default(),
    );

    // Raw: the engine returns the declared shape, not `{ id }`.
    let raw = engine
        .call(
            "/m/place_order",
            json!({ "org": "o-1", "status": "open", "total": 7 }),
            json!({}),
        )
        .await;
    assert_eq!(raw.status, 200);
    assert_eq!(raw.body, json!({ "status": "open", "total": 7 }));

    // Typed: `place_order` returns `OrderCard`, and the shaped body decodes into it —
    // the same typed round-trip a `get` gets.
    let api = client::embedded(&engine);
    let card = api
        .place_order(
            client::PlaceOrderInput {
                org: client::Id::from_raw("o-2"),
                status: "open".into(),
                total: 7,
            },
            (),
        )
        .await
        .expect("write response decodes into the declared OrderCard ");
    assert_eq!(card.status, "open");
    assert_eq!(card.total, 7);
}

/// A `-> stream` query on the typed surface: same method name, a `RowStream<OrderCard>`
/// back, one typed row per item — the engine's shaped rows decoded in-process, no
/// socket and no NDJSON framing.
#[tokio::test]
async fn typed_stream_yields_rows_in_process() {
    use futures_util::StreamExt;

    let db = MockDb::new(vec![vec![
        row(json!({ "status": "paid", "total": 9 })),
        row(json!({ "status": "open", "total": 3 })),
    ]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let mut rows = api
        .export_orders(
            client::ExportOrdersInput {
                org: client::Id::from_raw("org-1"),
            },
            (),
        )
        .await
        .expect("the stream starts");
    let mut cards: Vec<client::OrderCard> = Vec::new();
    while let Some(card) = rows.next().await {
        cards.push(card.expect("row decodes"));
    }
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].status, "paid");
    assert_eq!(cards[0].total, 9);
    assert_eq!(cards[1].status, "open");
}

/// A mid-pass database failure is the stream's final item: a typed `Err` carrying the
/// same stable code the wire's in-band `error` line does; after it the stream is over.
#[tokio::test]
async fn typed_stream_surfaces_a_mid_stream_error_item() {
    use futures_util::StreamExt;

    let db = MockDb::failing_mid_stream(
        vec![row(json!({ "status": "paid", "total": 9 }))],
        "connection lost",
    );
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let mut rows = api
        .export_orders(
            client::ExportOrdersInput {
                org: client::Id::from_raw("org-1"),
            },
            (),
        )
        .await
        .expect("the stream starts");
    let first = rows.next().await.expect("has a first row");
    assert!(first.is_ok());
    let err = rows
        .next()
        .await
        .expect("the failure arrives as an item")
        .expect_err("a mid-stream failure is an Err item");
    assert_eq!(err.code(), "database_error");
    assert_eq!(err.status(), Some(503));
    assert!(err.message().contains("connection lost"));
    assert!(rows.next().await.is_none(), "the stream ends after an Err");
}

/// A pre-body rejection never starts the stream: the outer `Err` carries the same
/// status + code the wire would send (here a missing required arg → 400).
#[tokio::test]
async fn typed_stream_pre_body_rejection_is_the_outer_err() {
    let engine = Engine::new(compiled(), MockDb::new(vec![]), SeqIdGen::default());
    let resp = engine
        .call_stream("/q/export_orders", json!({}), json!({}))
        .await
        .err()
        .expect("missing arg is rejected before the stream");
    assert_eq!(resp.status, 400);
    assert_eq!(resp.body["error"]["code"], "missing_arg");
}

/// Drop = cancel on the typed surface, proven against a real engine: the backend pool
/// holds exactly one connection, so the follow-up typed mutation only succeeds if
/// dropping the `RowStream` mid-pass returned that connection healthy.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn dropping_the_typed_stream_releases_the_connection() {
    use based_codegen::{sql, Dialect};
    use based_runtime::SqliteBackend;
    use futures_util::StreamExt;

    let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, _) = based_sema::check(&sf.decls);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    backend
        .execute_batch(&sql::ddl(&compiled.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL");
    backend
        .execute_batch(
            r#"
            INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme');
            INSERT INTO `order` (`id`, `org_id`, `status`, `total`) VALUES
                ('o-1', 'org-1', 'open', 10),
                ('o-2', 'org-1', 'paid', 30);
            "#,
        )
        .await
        .expect("seed fixtures");

    let engine = Engine::new(compiled, backend, SeqIdGen::default());
    let api = client::embedded(&engine);
    let input = || client::ExportOrdersInput {
        org: client::Id::from_raw("org-1"),
    };

    // Take one row, then drop the typed stream mid-pass (the caller cancelled).
    let mut rows = api.export_orders(input(), ()).await.expect("stream starts");
    let first = rows
        .next()
        .await
        .expect("has a first row")
        .expect("decodes");
    assert_eq!(first.total, 30);
    drop(rows);

    // The single pooled connection must be back and clean: a typed write runs green.
    let card = api
        .place_order(
            client::PlaceOrderInput {
                org: client::Id::from_raw("org-1"),
                status: "new".into(),
                total: 5,
            },
            (),
        )
        .await
        .expect("the pool serves a mutation after the cancelled stream");
    assert_eq!(card.status, "new");

    // And a fresh full pass sees every row, including the just-written one.
    let full: Vec<_> = api
        .export_orders(input(), ())
        .await
        .expect("stream starts on the recycled connection")
        .collect()
        .await;
    assert_eq!(full.len(), 3);
    assert!(full.iter().all(|r| r.is_ok()));
}
