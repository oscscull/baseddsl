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
use based_runtime::{Compiled, Engine, GuardVerdict, Guards, MockDb, Row, SeqIdGen};
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

/// A `with count` page round-trips its total through the typed client: the engine runs
/// the second COUNT statement and the decoded `Page` carries `total: Some(n)`.
#[tokio::test]
async fn typed_counted_page_carries_total() {
    // Row batch, then the COUNT batch the `with count` plan runs.
    let db = MockDb::new(vec![
        vec![row(json!({ "status": "paid", "total": 9 }))],
        vec![row(json!({ "count": 57 }))],
    ]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let page = api
        .counted_order_page(
            client::CountedOrderPageInput {
                org: client::Id::from_raw("org-1"),
                offset: None,
            },
            (),
        )
        .await
        .expect("call ok");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.total, Some(57));
}

/// A page without `with count` decodes to `total: None` — the wire omits the field.
#[tokio::test]
async fn typed_uncounted_page_has_no_total() {
    // A short page (1 row < page size 2): no next cursor, and no COUNT statement runs.
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 9 }))]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::embedded(&engine);

    let page = api
        .order_page(
            client::OrderPageInput {
                org: client::Id::from_raw("org-1"),
                cursor: None,
            },
            (),
        )
        .await
        .expect("call ok");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.total, None);
    assert!(page.cursor.is_none());
}

/// `$ctx` is a **typed** argument on the generated method: `my_org_orders` takes a
/// `MyOrgOrdersCtx { org }`. With the required context the `$ctx`-scoped query runs; an
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

/// The typed keyed door in-process: `place_order_with_key` rides
/// `Engine::call_with_key`, so a retry with the same key replays the first attempt's
/// recorded response — two identical typed results, exactly one transaction.
#[tokio::test]
async fn keyed_mutation_replays_in_process() {
    let db = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let engine = Engine::new(compiled(), db.clone(), SeqIdGen::default());
    let api = client::embedded(&engine);
    let input = || client::PlaceOrderInput {
        org: client::Id::from_raw("o-1"),
        status: "open".into(),
        total: 7,
    };

    let first = api
        .place_order_with_key(input(), (), "key-embed-1")
        .await
        .expect("the first keyed write runs");
    let second = api
        .place_order_with_key(input(), (), "key-embed-1")
        .await
        .expect("the retry replays");
    assert_eq!(first.status, second.status);
    assert_eq!(first.total, second.total);
    assert_eq!(
        db.tx_log(),
        vec!["begin", "commit"],
        "the retry must never open a second transaction"
    );
}

// ---------- guards (auth.md Handle 3) ---------------------------------------

/// A schema whose one mutation declares a host guard.
const GUARDED_SCHEMA: &str = r#"
    Order { status: text, total: int }
    shape OrderCard from Order { status, total }
    query order_by_id(id) -> OrderCard;
    mutation close_order(id) -> OrderCard guard caller_can_close {
        update Order where (id = $id) { status = "closed" };
    }
"#;

fn guarded_compiled() -> Compiled {
    let sf = based_parser::parse_file(GUARDED_SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = based_sema::check(&sf.decls);
    assert!(!diags
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error));
    Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
}

/// The registered guard decides per request: an allowed caller's write runs, a denied
/// caller gets the `403 guard_denied` with the guard's reason — through the same
/// in-process door every embedded typed call takes.
#[tokio::test]
async fn registered_guard_allows_and_denies_in_process() {
    let db = MockDb::new(vec![vec![row(json!({ "status": "closed", "total": 9 }))]]);
    let guards = Guards::new().register("caller_can_close", |req| async move {
        if req.ctx["role"] == "agent" {
            GuardVerdict::Allow
        } else {
            GuardVerdict::deny("only agents may close orders")
        }
    });
    let engine = Engine::with_guards(guarded_compiled(), db.clone(), SeqIdGen::default(), guards)
        .expect("every declared guard is registered");

    let allowed = engine
        .call(
            "/m/close_order",
            json!({ "id": "o-1" }),
            json!({ "role": "agent" }),
        )
        .await;
    assert_eq!(allowed.status, 200, "{:?}", allowed.body);

    let denied = engine
        .call(
            "/m/close_order",
            json!({ "id": "o-1" }),
            json!({ "role": "requester" }),
        )
        .await;
    assert_eq!(denied.status, 403);
    assert_eq!(denied.body["error"]["code"], "guard_denied");
    assert_eq!(
        denied.body["error"]["message"],
        "only agents may close orders"
    );
    // Exactly the allowed call's transaction ran; the denial wrote nothing.
    assert_eq!(db.tx_log(), vec!["begin", "commit"]);
}

/// A guard may call back into the engine that invoked it: dispatch holds no
/// engine-wide lock, so the re-entrant read completes and the guarded write runs.
/// Bounded by a timeout so a regression fails fast instead of hanging the suite.
#[tokio::test]
async fn guard_reenters_its_own_engine() {
    use std::sync::{Arc, OnceLock};

    // Result set 0 feeds the guard's re-entrant read; set 1 the mutation's re-select.
    let db = MockDb::new(vec![
        vec![row(json!({ "status": "resolved", "total": 9 }))],
        vec![row(json!({ "status": "closed", "total": 9 }))],
    ]);
    let slot: Arc<OnceLock<Arc<Engine>>> = Arc::new(OnceLock::new());
    let guards = Guards::new().register("caller_can_close", {
        let slot = Arc::clone(&slot);
        move |req| {
            let slot = Arc::clone(&slot);
            async move {
                let engine = slot.get().expect("engine is set before any call");
                let read = engine
                    .call(
                        "/q/order_by_id",
                        json!({ "id": req.args["id"].clone() }),
                        json!({}),
                    )
                    .await;
                if read.status == 200 && read.body["status"] == "resolved" {
                    GuardVerdict::Allow
                } else {
                    GuardVerdict::deny("only a resolved order can be closed")
                }
            }
        }
    });
    let engine = Arc::new(
        Engine::with_guards(guarded_compiled(), db.clone(), SeqIdGen::default(), guards)
            .expect("every declared guard is registered"),
    );
    assert!(slot.set(Arc::clone(&engine)).is_ok());

    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.call("/m/close_order", json!({ "id": "o-1" }), json!({})),
    )
    .await
    .expect("guard re-entry must complete, not deadlock");
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body["status"], "closed");
    // The guard's read ran outside the write's transaction; exactly one tx ran.
    assert_eq!(db.tx_log(), vec!["begin", "commit"]);
}

/// A declared guard nobody registered fails when the engine is *built* — never a
/// silent pass at request time.
#[tokio::test]
async fn unregistered_guard_fails_at_engine_build() {
    let err = Engine::with_guards(
        guarded_compiled(),
        MockDb::new(vec![]),
        SeqIdGen::default(),
        Guards::new(),
    )
    .err()
    .expect("a guarded schema with no registered guard must not build");
    assert_eq!(
        err.missing,
        vec![("close_order".to_string(), "caller_can_close".to_string())]
    );
    assert!(err.to_string().contains("caller_can_close"));
}

/// The guard-free convenience constructor refuses a guarded schema loudly.
#[tokio::test]
#[should_panic(expected = "caller_can_close")]
async fn engine_new_panics_on_a_guarded_schema() {
    let _ = Engine::new(guarded_compiled(), MockDb::new(vec![]), SeqIdGen::default());
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

/// Keyed replay against a real database: a retried `place_order_with_key` returns the
/// first attempt's row and inserts **nothing** — the row count proves no double effect,
/// not just an equal-looking response.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn keyed_mutation_has_no_double_effect_on_live_sqlite() {
    use based_codegen::{sql, Dialect};
    use based_runtime::SqliteBackend;

    let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, _) = based_sema::check(&sf.decls);
    let compiled = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    backend
        .execute_batch(&sql::ddl(&compiled.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL");
    backend
        .execute_batch("INSERT INTO `org` (`id`, `name`) VALUES ('org-1', 'Acme');")
        .await
        .expect("seed fixtures");

    let engine = Engine::new(compiled, backend, SeqIdGen::default());
    let api = client::embedded(&engine);
    let input = || client::PlaceOrderInput {
        org: client::Id::from_raw("org-1"),
        status: "open".into(),
        total: 7,
    };

    let first = api
        .place_order_with_key(input(), (), "key-live-1")
        .await
        .expect("the first keyed write runs");
    let second = api
        .place_order_with_key(input(), (), "key-live-1")
        .await
        .expect("the retry replays");
    assert_eq!(first.status, second.status);
    assert_eq!(first.total, second.total);

    // Exactly one row landed in the real database.
    let rows = api
        .orders_in_org(
            client::OrdersInOrgInput {
                org: client::Id::from_raw("org-1"),
            },
            (),
        )
        .await
        .expect("list runs");
    assert_eq!(rows.len(), 1, "the retry must not insert a second row");
}

/// The `-> ok` acknowledgement against a real database, through the typed client:
/// `purge_order` (a `hard delete`) returns `Ok(())` and the row is really gone; the
/// same call again — the row no longer exists — is the engine's `404 not_found`,
/// surfaced as a typed `ClientError` with the stable code.
#[cfg(feature = "sqlite")]
#[tokio::test]
async fn ack_delete_round_trips_and_missing_row_is_not_found_on_live_sqlite() {
    use based_codegen::{sql, Dialect};
    use based_runtime::SqliteBackend;

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
                ('o-1', 'org-1', 'open', 10);
            "#,
        )
        .await
        .expect("seed fixtures");

    let engine = Engine::new(compiled, backend, SeqIdGen::default());
    let api = client::embedded(&engine);

    // The purge acknowledges with unit, and the row is really gone.
    api.purge_order(
        client::PurgeOrderInput {
            id: client::Id::from_raw("o-1"),
        },
        (),
    )
    .await
    .expect("the hard delete acknowledges");
    let rows = api
        .orders_in_org(
            client::OrdersInOrgInput {
                org: client::Id::from_raw("org-1"),
            },
            (),
        )
        .await
        .expect("list runs");
    assert!(rows.is_empty(), "the purged row must be gone");

    // Purging it again matches nothing: a typed 404 with the stable code.
    let err = api
        .purge_order(
            client::PurgeOrderInput {
                id: client::Id::from_raw("o-1"),
            },
            (),
        )
        .await
        .expect_err("a missing row is a not-found, never an empty success");
    assert_eq!(err.status(), Some(404), "{err}");
    assert_eq!(err.code(), "not_found");
}
