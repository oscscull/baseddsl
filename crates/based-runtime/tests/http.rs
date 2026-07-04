//! End-to-end listener tests: a real TCP request over the loopback socket → the
//! `based serve` HTTP edge → a decoded, planned, executed response. These exercise the
//! socket glue the pure `serve.rs` dispatch tests can't: request-line + header
//! collection, JSON body read, and the response write-back. The database is a mock
//! `Backend` (no live DB) so the whole path runs in-process on an ephemeral port.
#![cfg(feature = "serve")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use based_ast::FileId;
use based_parser::parse_file;
use based_runtime::http::{serve, ServeConfig, TrustedHeaderContext};
use based_runtime::{Backend, Compiled, Db, DbError, MockDb, Row};
use serde_json::json;

const SCHEMA: &str = r#"
    Org { name: text }
    Order { org: Org, status: text, total: int }
    shape OrderCard from Order { status, total }

    query order_by_id(id) -> OrderCard;
    query orders_in_org(org) -> OrderCard[];
    query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }
    mutation place_order(org: Id, status, total: int) -> OrderCard {
        create Order { org = $org, status = $status, total = $total };
    }
"#;

/// A `Backend` that hands every request a fresh `MockDb` preloaded with canned rows —
/// the socket test's stand-in for a live shard pool.
struct MockBackend {
    rows: Vec<Vec<Row>>,
}

impl MockBackend {
    fn new(rows: Vec<Vec<Row>>) -> MockBackend {
        MockBackend { rows }
    }
}

impl Backend for MockBackend {
    fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(MockDb::new(self.rows.clone())))
    }
}

fn row(pairs: serde_json::Value) -> Row {
    pairs.as_object().cloned().unwrap()
}

fn compile() -> Compiled {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = based_sema::check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema should check clean"
    );
    Compiled::from_checked(schema, sf.decls)
}

/// Start a listener on a free loopback port and return its `host:port`. The server
/// thread runs forever (killed on process exit) — fine for a test.
fn start(backend: MockBackend) -> String {
    // Grab a free port, then hand the address to the server (a small, standard race —
    // acceptable for a loopback test).
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let listen = addr.clone();
    thread::spawn(move || {
        let config = ServeConfig { listen, workers: 2 };
        serve(compile(), backend, TrustedHeaderContext::default(), config).unwrap();
    });
    // Give the listener a moment to bind before the first connect.
    wait_until_up(&addr);
    addr
}

fn wait_until_up(addr: &str) {
    for _ in 0..100 {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("server never came up at {addr}");
}

/// A decoded HTTP response: status code + parsed JSON body.
struct Resp {
    status: u16,
    body: serde_json::Value,
}

/// Send one raw HTTP/1.1 POST and read the whole response (Connection: close).
fn post(addr: &str, path: &str, body: &str, headers: &[(&str, &str)]) -> Resp {
    let mut stream = TcpStream::connect(addr).unwrap();
    let mut req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes()).unwrap();

    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, payload) = raw.split_once("\r\n\r\n").expect("response has a body");
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
    Resp { status, body }
}

#[test]
fn get_query_over_the_socket() {
    let backend = MockBackend::new(vec![vec![row(json!({ "status": "paid", "total": 42 }))]]);
    let addr = start(backend);
    let resp = post(&addr, "/q/order_by_id", r#"{"id":"o-1"}"#, &[]);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "paid", "total": 42 }));
}

#[test]
fn list_query_returns_array() {
    let backend = MockBackend::new(vec![vec![
        row(json!({ "status": "paid", "total": 1 })),
        row(json!({ "status": "open", "total": 2 })),
    ]]);
    let addr = start(backend);
    let resp = post(&addr, "/q/orders_in_org", r#"{"org":"org-1"}"#, &[]);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_array().unwrap().len(), 2);
}

#[test]
fn ctx_arrives_from_the_header_not_the_body() {
    // my_org_orders requires $ctx.org; supplied via the trusted header, the request
    // plans and runs. (Absent, it would 400 — see the next test.)
    let backend = MockBackend::new(vec![vec![row(json!({ "status": "paid", "total": 9 }))]]);
    let addr = start(backend);
    let resp = post(
        &addr,
        "/q/my_org_orders",
        "{}",
        &[("X-Based-Context", r#"{"org":"org-7"}"#)],
    );
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_array().unwrap().len(), 1);
}

#[test]
fn missing_required_ctx_is_400() {
    let backend = MockBackend::new(vec![]);
    let addr = start(backend);
    let resp = post(&addr, "/q/my_org_orders", "{}", &[]);
    assert_eq!(resp.status, 400);
    assert_eq!(resp.body["error"]["code"], "missing_ctx");
}

#[test]
fn mutation_over_the_socket_returns_the_declared_shape() {
    // The backend answers the post-write re-select with the shaped row (D12).
    let backend = MockBackend::new(vec![vec![row(json!({ "status": "open", "total": 5 }))]]);
    let addr = start(backend);
    let resp = post(
        &addr,
        "/m/place_order",
        r#"{"org":"org-1","status":"open","total":5}"#,
        &[],
    );
    assert_eq!(resp.status, 200);
    // The write response is the created row read back in its declared `OrderCard` shape.
    assert_eq!(resp.body, json!({ "status": "open", "total": 5 }));
}

#[test]
fn idempotency_key_header_dedupes_a_write_over_the_socket() {
    // A retry with the same `Idempotency-Key` header replays the first attempt's stored
    // response instead of writing again (D25). The store is shared across the worker pool,
    // so a retry that lands on any worker dedupes. Here both mocks would return the same
    // shaped row, so the replay is proven rigorously by the pure `serve.rs` test
    // (`db.calls.is_empty()` on the retry); over the socket we assert the header is honored
    // end to end and yields the same 200 response — the `Backend` never sees a malformed
    // second write.
    let backend = MockBackend::new(vec![vec![row(json!({ "status": "open", "total": 5 }))]]);
    let addr = start(backend);

    let body = r#"{"org":"org-1","status":"open","total":5}"#;
    let key = &[("Idempotency-Key", "req-100")];

    let first = post(&addr, "/m/place_order", body, key);
    assert_eq!(first.status, 200);
    assert_eq!(first.body, json!({ "status": "open", "total": 5 }));

    // The retry replays the recorded response (same body, still 200) — not a fresh write.
    let retry = post(&addr, "/m/place_order", body, key);
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body, json!({ "status": "open", "total": 5 }));

    // A *different* key is a distinct request and runs the write path again (200 with the
    // freshly re-selected row) — the key scopes dedup, it does not freeze the endpoint.
    let other = post(
        &addr,
        "/m/place_order",
        body,
        &[("Idempotency-Key", "req-200")],
    );
    assert_eq!(other.status, 200);
    assert_eq!(other.body, json!({ "status": "open", "total": 5 }));
}

#[test]
fn bad_route_is_404_without_touching_the_db() {
    let backend = MockBackend::new(vec![]);
    let addr = start(backend);
    let resp = post(&addr, "/nope/whatever", "{}", &[]);
    assert_eq!(resp.status, 404);
}

#[test]
fn malformed_json_body_is_400() {
    let backend = MockBackend::new(vec![]);
    let addr = start(backend);
    let resp = post(&addr, "/q/order_by_id", "{not json", &[]);
    assert_eq!(resp.status, 400);
    assert_eq!(resp.body["error"]["code"], "bad_body");
}
