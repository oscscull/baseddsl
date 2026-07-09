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

/// Verbatim `based gen client` output for `SCHEMA` (target: rust, `embedded: true`). Do
/// not edit by hand; regenerate with the embedded option if `SCHEMA` changes.
#[allow(dead_code)]
mod client {
    use serde::{Deserialize, Serialize};
    use std::marker::PhantomData;

    // Semantic aliases for the wire types (mirrors the DDL mapping).
    pub type Uuid = String;
    pub type Timestamp = String;
    pub type Date = String;
    pub type Json = serde_json::Value;

    /// A typed id: the primary key of entity `E`, carried on the wire as its raw string
    /// (`#[serde(transparent)]`, so the wire is unchanged). The `E` marker keeps ids of
    /// different entities distinct types, so a `User` id can't be passed where an `Org` id
    /// is wanted. A `create_*` result already hands one back typed; turn a raw string into
    /// one only through the explicit, greppable `Id::from_raw`.
    #[derive(Serialize, Deserialize)]
    #[serde(transparent, bound = "")]
    pub struct Id<E> {
        raw: String,
        #[serde(skip)]
        _entity: PhantomData<fn() -> E>,
    }

    impl<E> Id<E> {
        /// Wrap a raw id string as a typed id — the explicit escape from an untyped string,
        /// used only where the string's entity is known (an id from outside the client).
        pub fn from_raw(raw: impl Into<String>) -> Self {
            Id {
                raw: raw.into(),
                _entity: PhantomData,
            }
        }
        /// The underlying id string.
        pub fn as_str(&self) -> &str {
            &self.raw
        }
        /// Consume into the raw id string.
        pub fn into_raw(self) -> String {
            self.raw
        }
    }

    // Hand-written so the marker `E` carries no trait bounds (a derive would demand
    // `E: Clone`, `E: Ord`, … of a type that only ever tags).
    impl<E> Clone for Id<E> {
        fn clone(&self) -> Self {
            Id {
                raw: self.raw.clone(),
                _entity: PhantomData,
            }
        }
    }
    impl<E> std::fmt::Debug for Id<E> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Id({:?})", self.raw)
        }
    }
    impl<E> std::fmt::Display for Id<E> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.raw)
        }
    }
    impl<E> PartialEq for Id<E> {
        fn eq(&self, other: &Self) -> bool {
            self.raw == other.raw
        }
    }
    impl<E> Eq for Id<E> {}
    impl<E> PartialOrd for Id<E> {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl<E> Ord for Id<E> {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.raw.cmp(&other.raw)
        }
    }
    impl<E> std::hash::Hash for Id<E> {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            self.raw.hash(state);
        }
    }

    /// An opaque keyset pagination cursor, carried on the wire as its underlying string
    /// (`#[serde(transparent)]`). A page hands one back; the caller feeds it to the next call.
    #[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct Cursor(String);

    impl Cursor {
        pub fn from_raw(raw: impl Into<String>) -> Self {
            Cursor(raw.into())
        }
        pub fn as_str(&self) -> &str {
            &self.0
        }
        pub fn into_raw(self) -> String {
            self.0
        }
    }

    impl std::fmt::Debug for Cursor {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Cursor({:?})", self.0)
        }
    }
    impl std::fmt::Display for Cursor {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    /// Pagination envelope: a paginated query returns rows + an opaque cursor.
    /// Next page = the same call carrying `cursor`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Page<T> {
        pub rows: Vec<T>,
        pub cursor: Option<Cursor>,
    }

    /// What went wrong in a client call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ClientErrorKind {
        Transport,
        Decode,
        Api { status: u16, code: String },
    }

    /// An error from a client call: transport failure, decode failure, or a structured
    /// server error. Mirrors `based gen client` output (verified by the regen gate).
    #[derive(Debug, Clone)]
    pub struct ClientError {
        kind: ClientErrorKind,
        message: String,
        source: Option<std::sync::Arc<dyn std::error::Error + Send + Sync + 'static>>,
    }

    impl ClientError {
        pub fn decode(err: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
            let err = err.into();
            ClientError {
                kind: ClientErrorKind::Decode,
                message: err.to_string(),
                source: Some(err.into()),
            }
        }
        pub fn api(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
            ClientError {
                kind: ClientErrorKind::Api {
                    status,
                    code: code.into(),
                },
                message: message.into(),
                source: None,
            }
        }
        pub fn code(&self) -> &str {
            match &self.kind {
                ClientErrorKind::Transport => "transport",
                ClientErrorKind::Decode => "decode",
                ClientErrorKind::Api { code, .. } => code,
            }
        }
        pub fn message(&self) -> &str {
            &self.message
        }
    }

    impl std::fmt::Display for ClientError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }
    impl std::error::Error for ClientError {}

    /// Post a typed input to a route, carry the typed request context (`$ctx`, carried out
    /// of band as request context), and decode the typed output. A callable with no `$ctx`
    /// requirements passes `ctx: &()`. Implemented by the runtime's HTTP client; codegen
    /// only depends on this shape.
    pub trait Transport {
        fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
        where
            I: Serialize,
            C: Serialize,
            O: serde::de::DeserializeOwned;
    }

    /// The generated client, generic over a `Transport`.
    pub struct Client<T> {
        pub transport: T,
    }

    /// Phantom entity tags for `Id<entity::M>` (types only, never constructed).
    pub mod entity {
        pub enum Org {}
        pub enum Order {}
    }

    // ---------- output types ----------

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OrderCard {
        pub status: String,
        pub total: i64,
    }

    // ---------- inputs + routes ----------

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OrderByIdInput {
        pub id: Id<entity::Order>,
    }
    /// Wire route for `order_by_id`.
    pub const ORDER_BY_ID_ROUTE: &str = "/q/order_by_id";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OrdersInOrgInput {
        pub org: Id<entity::Org>,
    }
    /// Wire route for `orders_in_org`.
    pub const ORDERS_IN_ORG_ROUTE: &str = "/q/orders_in_org";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MyOrgOrdersInput;
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MyOrgOrdersCtx {
        pub org: Id<entity::Org>,
    }
    /// Wire route for `my_org_orders`.
    pub const MY_ORG_ORDERS_ROUTE: &str = "/q/my_org_orders";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PlaceOrderInput {
        pub org: Id<entity::Org>,
        pub status: String,
        pub total: i64,
    }
    /// Wire route for `place_order`.
    pub const PLACE_ORDER_ROUTE: &str = "/m/place_order";

    // ---------- client ----------

    impl<T: Transport> Client<T> {
        /// `POST /q/order_by_id`
        pub fn order_by_id(
            &self,
            input: OrderByIdInput,
            ctx: (),
        ) -> Result<Option<OrderCard>, ClientError> {
            self.transport.call(ORDER_BY_ID_ROUTE, &input, &ctx)
        }
        /// `POST /q/orders_in_org`
        pub fn orders_in_org(
            &self,
            input: OrdersInOrgInput,
            ctx: (),
        ) -> Result<Vec<OrderCard>, ClientError> {
            self.transport.call(ORDERS_IN_ORG_ROUTE, &input, &ctx)
        }
        /// `POST /q/my_org_orders`
        pub fn my_org_orders(
            &self,
            input: MyOrgOrdersInput,
            ctx: MyOrgOrdersCtx,
        ) -> Result<Vec<OrderCard>, ClientError> {
            self.transport.call(MY_ORG_ORDERS_ROUTE, &input, &ctx)
        }
        /// `POST /m/place_order`
        pub fn place_order(
            &self,
            input: PlaceOrderInput,
            ctx: (),
        ) -> Result<OrderCard, ClientError> {
            self.transport.call(PLACE_ORDER_ROUTE, &input, &ctx)
        }
    }

    // ---------- embedded bridge (based_runtime::Engine) ----------

    /// A `Transport` backed by an in-process `based_runtime::Engine` — every callable runs
    /// through the engine's dispatch core with no socket. Build one with [`embedded`].
    pub struct Embedded<'a> {
        engine: &'a based_runtime::Engine,
    }

    impl Transport for Embedded<'_> {
        fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
        where
            I: Serialize,
            C: Serialize,
            O: serde::de::DeserializeOwned,
        {
            let args = serde_json::to_value(input).map_err(ClientError::decode)?;
            // `&()` → JSON `null`; the engine treats a non-object context as empty.
            let ctx = serde_json::to_value(ctx)
                .map(|v| {
                    if v.is_object() {
                        v
                    } else {
                        serde_json::json!({})
                    }
                })
                .map_err(ClientError::decode)?;
            let resp = self.engine.call(route, args, ctx);
            if resp.status == 200 {
                serde_json::from_value(resp.body).map_err(ClientError::decode)
            } else {
                let code = resp.body["error"]["code"].as_str().unwrap_or("error");
                let message = resp.body["error"]["message"]
                    .as_str()
                    .unwrap_or("call failed");
                Err(ClientError::api(resp.status, code, message))
            }
        }
    }

    /// A ready-to-use client over an in-process `based_runtime::Engine` — no bridge to write.
    /// `$ctx` is a typed per-call argument the app sets, not the caller; a public callable
    /// passes `()`, which maps to an empty context bag.
    pub fn embedded(engine: &based_runtime::Engine) -> Client<Embedded<'_>> {
        Client {
            transport: Embedded { engine },
        }
    }
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
