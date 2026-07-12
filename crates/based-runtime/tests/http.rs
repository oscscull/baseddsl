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
use based_runtime::http::{serve_with_handle, Handle, ServeConfig, TrustedHeaderContext};
use based_runtime::{Backend, Compiled, Db, DbError, MockDb, Row};
use serde_json::json;

const SCHEMA: &str = r#"
    Org { name: text }
    Order { org: Org, status: text, total: int }
    shape OrderCard from Order { status, total }

    query order_by_id(id) -> OrderCard;
    query orders_in_org(org) -> OrderCard[];
    query export_orders(org) -> stream OrderCard;
    query my_org_orders() -> OrderCard[] { list Order where (org = $ctx.org); }
    mutation place_order(org: Id, status, total: int) -> OrderCard {
        create Order { org = $org, status = $status, total = $total };
    }
"#;

/// A `Backend` that hands every request a fresh `MockDb` preloaded with canned rows —
/// the socket test's stand-in for a live shard pool. `ready` drives the readiness probe
/// (`Backend::ping`): `false` simulates an unreachable database.
struct MockBackend {
    rows: Vec<Vec<Row>>,
    ready: bool,
    /// When set, every connection's `fetch` yields its rows then this failure —
    /// the database breaking mid-stream.
    mid_stream_fail: Option<String>,
}

impl MockBackend {
    fn new(rows: Vec<Vec<Row>>) -> MockBackend {
        MockBackend {
            rows,
            ready: true,
            mid_stream_fail: None,
        }
    }

    /// A backend whose readiness probe fails (the DB-down case).
    fn not_ready() -> MockBackend {
        MockBackend {
            rows: vec![],
            ready: false,
            mid_stream_fail: None,
        }
    }

    /// A backend whose reads deliver `rows`, then fail with `message` mid-stream.
    fn failing_mid_stream(rows: Vec<Row>, message: &str) -> MockBackend {
        MockBackend {
            rows: vec![rows],
            ready: true,
            mid_stream_fail: Some(message.to_string()),
        }
    }
}

#[async_trait::async_trait]
impl Backend for MockBackend {
    async fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(match &self.mid_stream_fail {
            Some(m) => Box::new(MockDb::failing_mid_stream(
                self.rows.first().cloned().unwrap_or_default(),
                m.clone(),
            )),
            None => Box::new(MockDb::new(self.rows.clone())),
        })
    }

    async fn ping(&self) -> Result<(), DbError> {
        if self.ready {
            Ok(())
        } else {
            Err(DbError::new("shard unreachable"))
        }
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
    Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
}

/// Start a listener on a free loopback port and return its `host:port`. The server
/// thread runs forever (killed on process exit) — fine for a test.
fn start(backend: MockBackend) -> String {
    start_with_handle(backend).0
}

/// Like [`start`] but also returns the [`Handle`] (for the graceful-shutdown test) and
/// the server thread's `JoinHandle` (so the drain test can prove the call *returns*).
fn start_with_handle(backend: MockBackend) -> (String, Handle, thread::JoinHandle<()>) {
    // Grab a free port, then hand the address to the server (a small, standard race —
    // acceptable for a loopback test).
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let listen = addr.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Handle>();
    let server = thread::spawn(move || {
        let config = ServeConfig { listen };
        // The listener is async; the test drives it on its own runtime in this thread
        // (the client side stays plain blocking TCP).
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(serve_with_handle(
                compile(),
                backend,
                TrustedHeaderContext::default(),
                config,
                |handle| tx.send(handle).unwrap(),
            ))
            .unwrap();
    });
    // The handle is sent once the listener is up; receiving it means we're serving.
    let handle = rx.recv().unwrap();
    wait_until_up(&addr);
    (addr, handle, server)
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

/// A streaming response, undecoded: status, lowercased header pairs, and the
/// (de-chunked) body text — for asserting NDJSON framing line by line.
struct RawResp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl RawResp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == &name.to_ascii_lowercase())
            .map(|(_, v)| v.as_str())
    }
}

/// Send one raw HTTP/1.1 POST and return the response undecoded (body as text).
fn post_raw(addr: &str, path: &str, body: &str, headers: &[(&str, &str)]) -> RawResp {
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
    let headers: Vec<(String, String)> = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked"));
    let body = if chunked {
        dechunk(payload)
    } else {
        payload.to_string()
    };
    RawResp {
        status,
        headers,
        body,
    }
}

/// Decode an HTTP/1.1 chunked body: `<hex size>\r\n<chunk>\r\n` repeated, ended by a
/// zero-size chunk (a streamed axum body arrives chunked).
fn dechunk(payload: &str) -> String {
    let mut out = String::new();
    let mut rest = payload;
    while let Some((size_line, tail)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size_line.trim(), 16).unwrap_or(0);
        if size == 0 || tail.len() < size {
            break;
        }
        out.push_str(&tail[..size]);
        rest = tail.get(size + 2..).unwrap_or("");
    }
    out
}

/// Send one raw HTTP/1.1 GET (no body) and read the whole response — for the `/healthz`
/// and `/readyz` operational probes, which are GETs (the POST rule is for the
/// RPC wire; the probes are outside it).
fn get(addr: &str, path: &str) -> Resp {
    let mut stream = TcpStream::connect(addr).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
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
fn stream_query_returns_ndjson_with_the_terminal_done_line() {
    let backend = MockBackend::new(vec![vec![
        row(json!({ "status": "paid", "total": 1 })),
        row(json!({ "status": "open", "total": 2 })),
    ]]);
    let addr = start(backend);
    let resp = post_raw(&addr, "/q/export_orders", r#"{"org":"org-1"}"#, &[]);

    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/x-ndjson"));
    // Every line parses standalone; the last is the mandatory `done` with the row count.
    let lines: Vec<serde_json::Value> = resp
        .body
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON envelope"))
        .collect();
    assert_eq!(
        lines,
        vec![
            json!({ "row": { "status": "paid", "total": 1 } }),
            json!({ "row": { "status": "open", "total": 2 } }),
            json!({ "done": { "rows": 2 } }),
        ]
    );
}

#[test]
fn stream_mid_stream_failure_is_the_terminal_error_line() {
    // The status line is spent once the body starts: a database failure mid-stream
    // arrives in-band as the terminal `error` line, and no `done` follows.
    let backend = MockBackend::failing_mid_stream(
        vec![row(json!({ "status": "paid", "total": 1 }))],
        "connection lost",
    );
    let addr = start(backend);
    let resp = post_raw(&addr, "/q/export_orders", r#"{"org":"org-1"}"#, &[]);

    assert_eq!(resp.status, 200);
    let lines: Vec<serde_json::Value> = resp
        .body
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON envelope"))
        .collect();
    assert_eq!(
        lines,
        vec![
            json!({ "row": { "status": "paid", "total": 1 } }),
            json!({ "error": { "code": "database_error", "message": "connection lost" } }),
        ]
    );
}

#[test]
fn stream_pre_body_failure_keeps_its_real_http_status() {
    // Validation runs before the first byte of the body: a missing arg is the
    // ordinary JSON `400`, never a `200` NDJSON stream.
    let backend = MockBackend::new(vec![]);
    let addr = start(backend);
    let resp = post(&addr, "/q/export_orders", "{}", &[]);
    assert_eq!(resp.status, 400);
    assert_eq!(resp.body["error"]["code"], "missing_arg");
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
    // The backend answers the post-write re-select with the shaped row.
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
    // response instead of writing again. The store is shared across the worker pool,
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

#[test]
fn healthz_is_ok_and_touches_no_db() {
    // Liveness never consults the backend: an empty MockDb (no canned rows) still 200s.
    let addr = start(MockBackend::new(vec![]));
    let resp = get(&addr, "/healthz");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "ok" }));
}

#[test]
fn readyz_is_ok_when_the_backend_pings() {
    let addr = start(MockBackend::new(vec![]));
    let resp = get(&addr, "/readyz");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, json!({ "status": "ready" }));
}

#[test]
fn readyz_is_503_when_the_backend_is_down() {
    // A backend whose ping fails (DB unreachable) reports not-ready — the load balancer
    // pulls the instance out of rotation rather than restarting it (that's liveness).
    let addr = start(MockBackend::not_ready());
    let resp = get(&addr, "/readyz");
    assert_eq!(resp.status, 503);
    assert_eq!(resp.body["error"]["code"], "not_ready");
    // But liveness still passes — the process is up; the DB is the transient problem.
    assert_eq!(get(&addr, "/healthz").status, 200);
}

#[test]
fn graceful_shutdown_drains_and_returns() {
    // Handle::shutdown flips readiness to 503 (drain), lets in-flight requests finish,
    // and makes the serve call return — the container-story shutdown contract.
    let (addr, handle, server) = start_with_handle(MockBackend::new(vec![vec![row(
        json!({ "status": "paid", "total": 1 }),
    )]]));

    // Before shutdown: ready.
    assert_eq!(get(&addr, "/readyz").status, 200);

    handle.shutdown();

    // Readiness flips to draining. A worker still alive answers it (the drain window),
    // so poll briefly for the 503 rather than assuming instantaneous propagation.
    let mut saw_draining = false;
    for _ in 0..50 {
        if let Ok(mut s) = TcpStream::connect(&addr) {
            let _ = s
                .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let mut raw = String::new();
            if s.read_to_string(&mut raw).is_ok() {
                if let Some((head, _)) = raw.split_once("\r\n\r\n") {
                    if head.contains(" 503 ") {
                        saw_draining = true;
                        break;
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(saw_draining, "readiness should report 503 while draining");

    // The serve call returns once every worker has drained — the join proves the process
    // can exit cleanly (a hung serve would deadlock this test).
    server
        .join()
        .expect("serve thread should return after drain");
}
