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
/// wire shapes line up on both sides.
const SCHEMA: &str = r#"
    @soft_delete(deleted_at)
    Org { deleted_at: timestamp?, name: text }

    @soft_delete(deleted_at)
    @sort(total desc)
    Order {
        deleted_at: timestamp?,
        org: Org,
        status: text,
        total: int,
        @index(org)
    }
    shape OrderCard from Order { status, total }

    query order_by_id(id) -> OrderCard;
    query orders_in_org(org) -> OrderCard[];
    query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }

    mutation place_order(org: Id, status, total: int) -> OrderCard {
        create Order { org = $org, status = $status, total = $total };
    }
"#;

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
#[test]
fn typed_get_round_trips_in_process() {
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
        .expect("call ok");
    let card = got.expect("a row");
    assert_eq!(card.status, "paid");
    assert_eq!(card.total, 42);
}

/// A `get` that matches no row decodes to `None` (envelope `One` → JSON `null`).
#[test]
fn typed_get_missing_is_none() {
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
        .expect("call ok");
    assert!(got.is_none());
}

/// A typed `list` decodes into `Vec<OrderCard>`.
#[test]
fn typed_list_round_trips_in_process() {
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
#[test]
fn ctx_supplied_in_process_and_required() {
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
        .expect("call ok");
    assert_eq!(rows.len(), 1);

    // A route that requires `$ctx.org` but is reached with an empty context bag (the
    // untyped raw path here, mirroring a missing header) → a boundary 400.
    let err = engine.call("/q/my_org_orders", json!({}), json!({}));
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
#[test]
fn mutation_response_is_the_created_rows_declared_shape() {
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
    let raw = engine.call(
        "/m/place_order",
        json!({ "org": "o-1", "status": "open", "total": 7 }),
        json!({}),
    );
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
        .expect("write response decodes into the declared OrderCard ");
    assert_eq!(card.status, "open");
    assert_eq!(card.total, 7);
}
