//! Worked embed example (Tier 1): the **generated typed client** running in-process
//! over [`Engine`], with **no socket** — the library twin of `based serve`.
//!
//! This is the end-to-end proof of the in-process door. `mod client` below is the
//! *verbatim* output of `based gen client` for `SCHEMA` (committed so the test
//! exercises the real generated surface, not a hand-written stand-in). The `InProcess`
//! transport is the ~10-line bridge a Rust app writes: it lives here, next to the
//! generated module, because the `Transport` trait is defined *by* the generated code
//! (the orphan rule keeps the impl in the consumer's crate, not `based-runtime`).
//!
//! The payoff visible here: `client.order_by_id(...)` returns a typed
//! `Option<OrderCard>` decoded from the engine's shaped JSON — the same typed call an
//! HTTP client would make, minus the loopback socket + HTTP framing (D20: the win is
//! dropping the socket, not the JSON).

use based_ast::FileId;
use based_runtime::{Compiled, Engine, MockDb, Row, SeqIdGen};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;

/// Verbatim `based gen client` output for `SCHEMA` (target: rust). Do not edit by hand;
/// regenerate with `based gen client` if `SCHEMA` changes.
mod client {
    #![allow(dead_code)]

    use serde::{Deserialize, Serialize};

    pub type Uuid = String;
    pub type Timestamp = String;
    pub type Date = String;
    pub type Json = serde_json::Value;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Page<T> {
        pub rows: Vec<T>,
        pub cursor: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct ClientError(pub String);

    pub trait Transport {
        fn call<I, O>(&self, route: &str, input: &I) -> Result<O, ClientError>
        where
            I: Serialize,
            O: serde::de::DeserializeOwned;
    }

    pub struct Client<T> {
        pub transport: T,
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
        pub id: Uuid,
    }
    pub const ORDER_BY_ID_ROUTE: &str = "/q/order_by_id";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OrdersInOrgInput {
        pub org: Uuid,
    }
    pub const ORDERS_IN_ORG_ROUTE: &str = "/q/orders_in_org";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MyOrgOrdersInput;
    pub const MY_ORG_ORDERS_ROUTE: &str = "/q/my_org_orders";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PlaceOrderInput {
        pub org: Uuid,
        pub status: String,
        pub total: i64,
    }
    pub const PLACE_ORDER_ROUTE: &str = "/m/place_order";

    // ---------- client ----------

    impl<T: Transport> Client<T> {
        /// `POST /q/order_by_id`
        pub fn order_by_id(&self, input: OrderByIdInput) -> Result<Option<OrderCard>, ClientError> {
            self.transport.call(ORDER_BY_ID_ROUTE, &input)
        }
        /// `POST /q/orders_in_org`
        pub fn orders_in_org(
            &self,
            input: OrdersInOrgInput,
        ) -> Result<Vec<OrderCard>, ClientError> {
            self.transport.call(ORDERS_IN_ORG_ROUTE, &input)
        }
        /// `POST /q/my_org_orders`
        pub fn my_org_orders(
            &self,
            input: MyOrgOrdersInput,
        ) -> Result<Vec<OrderCard>, ClientError> {
            self.transport.call(MY_ORG_ORDERS_ROUTE, &input)
        }
        /// `POST /m/place_order`
        pub fn place_order(&self, input: PlaceOrderInput) -> Result<OrderCard, ClientError> {
            self.transport.call(PLACE_ORDER_ROUTE, &input)
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

/// The bridge: a `Transport` backed by an in-process [`Engine`]. This is the whole of
/// what an embedding app writes — serialize the typed input to JSON, run it through the
/// engine, decode the `200` body into the output type (a non-`200` → `ClientError`).
/// `$ctx` is held here (per unit-of-work), supplied straight in — no header dance.
struct InProcess<'a> {
    engine: &'a Engine,
    ctx: serde_json::Value,
}

impl client::Transport for InProcess<'_> {
    fn call<I, O>(&self, route: &str, input: &I) -> Result<O, client::ClientError>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let args = serde_json::to_value(input).map_err(|e| client::ClientError(e.to_string()))?;
        let resp = self.engine.call(route, args, self.ctx.clone());
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(|e| client::ClientError(e.to_string()))
        } else {
            let msg = resp.body["error"]["message"]
                .as_str()
                .unwrap_or("call failed");
            Err(client::ClientError(msg.to_string()))
        }
    }
}

fn compiled() -> Compiled {
    let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = based_sema::check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean"
    );
    Compiled::from_checked(schema, sf.decls)
}

fn row(v: serde_json::Value) -> Row {
    v.as_object().cloned().unwrap()
}

/// A typed `get` round-trips: the engine's shaped `200` decodes into `Option<OrderCard>`.
#[test]
fn typed_get_round_trips_in_process() {
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
    let engine = Engine::new(compiled(), db, SeqIdGen::default());
    let api = client::Client {
        transport: InProcess {
            engine: &engine,
            ctx: json!({}),
        },
    };

    let got = api
        .order_by_id(client::OrderByIdInput { id: "o-1".into() })
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
    let api = client::Client {
        transport: InProcess {
            engine: &engine,
            ctx: json!({}),
        },
    };

    let got = api
        .order_by_id(client::OrderByIdInput {
            id: "missing".into(),
        })
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
    let api = client::Client {
        transport: InProcess {
            engine: &engine,
            ctx: json!({}),
        },
    };

    let rows = api
        .orders_in_org(client::OrdersInOrgInput {
            org: "org-1".into(),
        })
        .expect("call ok");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].total, 9);
    assert_eq!(rows[1].status, "open");
}

/// `$ctx` is supplied straight in — no header dance (auth.md/D7). With the context the
/// engine requires, a `$ctx`-scoped query runs; without it, the boundary `400` maps to
/// the client's `ClientError` (the same non-200 an HTTP client would see).
#[test]
fn ctx_supplied_in_process_and_required() {
    let compiled = compiled();

    // With ctx.org present → the query runs and decodes.
    let db = MockDb::new(vec![vec![row(json!({ "status": "paid", "total": 1 }))]]);
    let engine = Engine::new(compiled, db, SeqIdGen::default());
    let api = client::Client {
        transport: InProcess {
            engine: &engine,
            ctx: json!({ "org": "org-9" }),
        },
    };
    let rows = api
        .my_org_orders(client::MyOrgOrdersInput)
        .expect("call ok");
    assert_eq!(rows.len(), 1);

    // Without it → a boundary 400 surfaces as ClientError (the missing-ctx message).
    let bare = client::Client {
        transport: InProcess {
            engine: &engine,
            ctx: json!({}),
        },
    };
    let err = bare
        .my_org_orders(client::MyOrgOrdersInput)
        .expect_err("missing ctx is an error");
    assert!(
        err.0.contains("ctx"),
        "message names the missing ctx: {}",
        err.0
    );
}

/// The write path runs in-process and returns the created row in its **declared shape**
/// (D12): after the INSERT the engine re-selects the created `Order` as an `OrderCard`,
/// still inside the transaction, and *that* is the `200` body — so the typed
/// `place_order` method decodes clean into an `OrderCard`, exactly like a `get`. This
/// closes the gap the earlier `{ id }` response left.
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
    let api = client::Client {
        transport: InProcess {
            engine: &engine,
            ctx: json!({}),
        },
    };
    let card = api
        .place_order(client::PlaceOrderInput {
            org: "o-2".into(),
            status: "open".into(),
            total: 7,
        })
        .expect("write response decodes into the declared OrderCard (D12)");
    assert_eq!(card.status, "open");
    assert_eq!(card.total, 7);
}
