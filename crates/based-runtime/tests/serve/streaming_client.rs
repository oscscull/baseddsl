//! The generated **streaming client** over real HTTP: the same committed generated
//! module `tests/embed.rs` uses (`tests/support/embedded_client.rs`), driven through a
//! reqwest-backed `Transport` against the `based serve` axum edge — so what runs is
//! the full pipeline a real consumer gets: typed method → `Transport::call_stream` →
//! NDJSON body → the emitted `decode_ndjson` framing decoder → typed rows.
//!
//! The gates: rows arrive typed and in order with the terminal `done` consumed; a
//! mid-stream database failure surfaces as the in-band `error` line = a typed `Err`
//! item carrying the server's stable code; a pre-body rejection keeps its real HTTP
//! status as the outer `Err`; and a body cut before the terminal line — a real socket
//! death and a pure chunk-stream twin — is a transport-kind `Err`, never completion.

#![cfg(feature = "serve")]

use based_ast::FileId;
use based_parser::parse_file;
use based_runtime::http::{serve_with_handle, ServeConfig, TrustedHeaderContext};
use based_runtime::{Backend, Compiled, Db, DbError, MockDb, Row};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::json;

/// The committed `based gen client` output for `SCHEMA` — the exact module a consumer
/// compiles (kept current by `embed.rs`'s `generated_client_is_current` gate).
#[allow(dead_code)]
mod client {
    include!("support/embedded_client.rs");
}

/// The schema the generated client was emitted from (shared with `tests/embed.rs`).
const SCHEMA: &str = include_str!("support/embedded_client_schema.bsl");

fn compile() -> Compiled {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = based_sema::check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260"),
        "schema should check clean"
    );
    Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
}

fn row(v: serde_json::Value) -> Row {
    v.as_object().cloned().unwrap()
}

/// A `Backend` handing every request a canned `MockDb`; `mid_stream_fail` makes each
/// read deliver its rows and then break — the database dying mid-stream.
struct MockBackend {
    rows: Vec<Vec<Row>>,
    mid_stream_fail: Option<String>,
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
}

/// Start the axum listener on a free loopback port (in this test's runtime) and
/// return its `host:port`. The server task lives until the test process exits.
async fn start(backend: impl Backend + 'static) -> String {
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let listen = addr.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        serve_with_handle(
            compile(),
            backend,
            TrustedHeaderContext,
            ServeConfig { listen },
            move |h| {
                let _ = tx.send(h);
            },
        )
        .await
        .unwrap();
    });
    // The handle arrives once the socket is bound and accepting.
    let _handle = rx.await.unwrap();
    addr
}

/// A real HTTP `Transport` for the generated client — a few lines over reqwest, the
/// shape any consumer's transport takes. The whole NDJSON framing contract comes from
/// the generated `decode_ndjson`, not from this impl.
struct Http {
    base: String,
    client: reqwest::Client,
}

impl Http {
    fn new(addr: &str) -> Self {
        Self {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
        }
    }

    /// Build one POST: JSON input body + the out-of-band `$ctx` header.
    fn post<I: Serialize, C: Serialize>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
    ) -> Result<reqwest::RequestBuilder, client::ClientError> {
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { json!({}) })
            .map_err(client::ClientError::decode)?;
        Ok(self
            .client
            .post(format!("{}{}", self.base, route))
            .header("X-Based-Context", ctx.to_string())
            .json(input))
    }
}

/// Rebuild the typed error from the wire's `{ error: { code, message } }` envelope.
fn api_error(status: u16, body: &serde_json::Value) -> client::ClientError {
    let code = body["error"]["code"].as_str().unwrap_or("error");
    let message = body["error"]["message"].as_str().unwrap_or("call failed");
    client::ClientError::api(status, code, message)
}

impl client::Transport for Http {
    async fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, client::ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        let resp = self
            .post(route, input, ctx)?
            .send()
            .await
            .map_err(client::ClientError::transport)?;
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.map_err(client::ClientError::transport)?;
        if status == 200 {
            serde_json::from_value(body).map_err(client::ClientError::decode)
        } else {
            Err(api_error(status, &body))
        }
    }

    /// The keyed door over HTTP: the idempotency key rides the standard
    /// `Idempotency-Key` header, out of band — the body is byte-identical to `call`'s.
    async fn call_with_key<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
        key: &str,
    ) -> Result<O, client::ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        let resp = self
            .post(route, input, ctx)?
            .header("Idempotency-Key", key)
            .send()
            .await
            .map_err(client::ClientError::transport)?;
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.map_err(client::ClientError::transport)?;
        if status == 200 {
            serde_json::from_value(body).map_err(client::ClientError::decode)
        } else {
            Err(api_error(status, &body))
        }
    }

    async fn call_stream<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
    ) -> Result<client::RowStream<O>, client::ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned + Send + 'static,
    {
        let resp = self
            .post(route, input, ctx)?
            .send()
            .await
            .map_err(client::ClientError::transport)?;
        let status = resp.status().as_u16();
        if status != 200 {
            // A pre-body rejection: an ordinary JSON error with its real status.
            let body: serde_json::Value =
                resp.json().await.map_err(client::ClientError::transport)?;
            return Err(api_error(status, &body));
        }
        // The generated decoder owns the framing contract from here.
        Ok(client::decode_ndjson(resp.bytes_stream()))
    }
}

fn http_client(addr: &str) -> client::Client<Http> {
    client::Client {
        transport: Http::new(addr),
    }
}

fn export_input() -> client::ExportOrdersInput {
    client::ExportOrdersInput {
        org: client::Id::from_raw("org-1"),
    }
}

// ---------- the gates, over the real socket ---------------------------------

/// The typed keyed door end-to-end over real HTTP: `place_order_with_key` sends the
/// `Idempotency-Key` header, so a retry with the same key replays the first attempt's
/// recorded response — two identical typed results, exactly one transaction.
#[tokio::test]
async fn keyed_mutation_rides_the_header_and_replays_over_real_http() {
    // `MockDb` is itself a `Backend` whose checkouts share one state, so the test can
    // keep a handle and count the transactions the server actually ran.
    let db = MockDb::new(vec![vec![row(json!({ "status": "open", "total": 7 }))]]);
    let addr = start(db.clone()).await;
    let api = http_client(&addr);
    let input = || client::PlaceOrderInput {
        org: client::Id::from_raw("org-1"),
        status: "open".into(),
        total: 7,
    };

    let first = api
        .place_order_with_key(input(), (), "key-http-1")
        .await
        .expect("the first keyed write runs");
    let second = api
        .place_order_with_key(input(), (), "key-http-1")
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

/// Happy path: typed rows in order off the NDJSON body, the terminal `done` consumed
/// (the stream simply ends Ok), nothing buffered into a Vec by the client machinery.
#[tokio::test]
async fn generated_http_client_streams_typed_rows() {
    let addr = start(MockBackend {
        rows: vec![vec![
            row(json!({ "status": "paid", "total": 9 })),
            row(json!({ "status": "open", "total": 3 })),
        ]],
        mid_stream_fail: None,
    })
    .await;
    let api = http_client(&addr);

    let mut rows = api
        .export_orders(export_input(), ())
        .await
        .expect("the stream starts");
    let mut cards: Vec<client::OrderCard> = Vec::new();
    while let Some(card) = rows.next().await {
        cards.push(card.expect("row decodes"));
    }
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].status, "paid");
    assert_eq!(cards[1].total, 3);
}

/// The 200 status line is spent when the database dies mid-stream; the failure rides
/// the in-band `error` line and surfaces as the typed `Err` item with the server's
/// stable code — after which the stream is finished.
#[tokio::test]
async fn mid_stream_db_error_arrives_as_the_in_band_error_item() {
    let addr = start(MockBackend {
        rows: vec![vec![row(json!({ "status": "paid", "total": 9 }))]],
        mid_stream_fail: Some("connection lost".into()),
    })
    .await;
    let api = http_client(&addr);

    let mut rows = api
        .export_orders(export_input(), ())
        .await
        .expect("the stream starts (the status line was already spent)");
    let first = rows.next().await.expect("first row arrives");
    assert!(first.is_ok());
    let err = rows
        .next()
        .await
        .expect("the failure arrives as an item")
        .expect_err("in-band error line is an Err item");
    assert_eq!(err.code(), "database_error");
    assert_eq!(err.status(), Some(503));
    assert!(err.message().contains("connection lost"));
    assert!(rows.next().await.is_none(), "the stream ends after an Err");
}

/// Validation runs before the first body byte: a missing required arg is the ordinary
/// JSON `400`, surfacing as the *outer* `Err` — the stream never starts.
#[tokio::test]
async fn pre_body_rejection_is_the_outer_err_with_its_real_status() {
    let addr = start(MockBackend {
        rows: vec![],
        mid_stream_fail: None,
    })
    .await;
    let api = http_client(&addr);

    // Bypass the typed input (it would not let the arg go missing) and post the same
    // route with an empty body through the transport directly.
    use client::Transport;
    let err = api
        .transport
        .call_stream::<_, _, client::OrderCard>(client::EXPORT_ORDERS_ROUTE, &json!({}), &())
        .await
        .err()
        .expect("missing arg is rejected before the stream");
    assert_eq!(err.status(), Some(400));
    assert_eq!(err.code(), "missing_arg");
}

/// Truncation over a real socket: the server dies after one row — the connection
/// closes with no terminal line — and the client reports a transport error, never
/// completion. The server here is a raw TCP listener so the death is exact.
#[tokio::test]
async fn truncated_socket_is_a_transport_error_never_completion() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut sock, _)) = listener.accept() {
            // Absorb the request head, answer with one row, then die mid-stream:
            // `Connection: close` + no Content-Length = a close-delimited body, so
            // the cut is invisible at the HTTP layer — only the missing terminal
            // line reveals it.
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n{\"row\":{\"status\":\"paid\",\"total\":9}}\n",
            );
        }
    });
    let api = http_client(&addr);

    let mut rows = api
        .export_orders(export_input(), ())
        .await
        .expect("the stream starts");
    let first = rows.next().await.expect("the delivered row arrives");
    assert_eq!(first.expect("row decodes").total, 9);
    let err = rows
        .next()
        .await
        .expect("the cut must surface as an item")
        .expect_err("a body without a terminal line is an error");
    assert_eq!(err.code(), "transport");
    assert!(rows.next().await.is_none(), "the stream ends after an Err");
}

// ---------- the framing decoder, pinned without a socket ---------------------

/// Feed the generated `decode_ndjson` hand-built chunk streams: the framing rules the
/// HTTP gates rely on, pinned deterministically (chunk boundaries mid-line, terminal
/// contract, checksum).
mod decoder {
    use super::*;

    fn chunks(parts: &[&str]) -> impl futures_core::Stream<Item = Result<Vec<u8>, std::io::Error>> {
        // The collect is load-bearing: the returned stream must not borrow `parts`, so the
        // owned bytes are materialized before `stream::iter` takes them.
        let owned: Vec<_> = parts.iter().map(|p| Ok(p.as_bytes().to_vec())).collect();
        futures_util::stream::iter(owned)
    }

    #[tokio::test]
    async fn reassembles_lines_across_chunk_boundaries() {
        // One row split mid-JSON across chunks, a second row and the terminal line
        // sharing a chunk — the decoder must see three envelopes, not the chunks.
        let rows: Vec<_> = client::decode_ndjson::<client::OrderCard, _, _, _>(chunks(&[
            "{\"row\":{\"status\":\"pa",
            "id\",\"total\":9}}\n{\"row\":{\"status\":\"open\",\"total\":3}}\n",
            "{\"done\":{\"rows\":2}}\n",
        ]))
        .collect()
        .await;
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(Result::is_ok));
        assert_eq!(rows[1].as_ref().unwrap().status, "open");
    }

    #[tokio::test]
    async fn body_ending_without_a_terminal_line_is_truncation() {
        let items: Vec<_> = client::decode_ndjson::<client::OrderCard, _, _, _>(chunks(&[
            "{\"row\":{\"status\":\"paid\",\"total\":9}}\n",
        ]))
        .collect()
        .await;
        assert_eq!(items.len(), 2);
        assert!(items[0].is_ok());
        let err = items[1].as_ref().unwrap_err();
        assert_eq!(err.code(), "transport");
        assert!(err.message().contains("terminal"), "{}", err.message());
    }

    #[tokio::test]
    async fn done_count_disagreement_is_reported() {
        // A lost row line with an intact terminal would otherwise pass silently;
        // `done.rows` is the checksum that catches it.
        let items: Vec<_> = client::decode_ndjson::<client::OrderCard, _, _, _>(chunks(&[
            "{\"row\":{\"status\":\"paid\",\"total\":9}}\n{\"done\":{\"rows\":2}}\n",
        ]))
        .collect()
        .await;
        let err = items[1].as_ref().unwrap_err();
        assert_eq!(err.code(), "transport");
        assert!(err.message().contains("checksum"), "{}", err.message());
    }

    #[tokio::test]
    async fn in_band_error_line_carries_the_stable_code() {
        let items: Vec<_> = client::decode_ndjson::<client::OrderCard, _, _, _>(chunks(&[
            "{\"row\":{\"status\":\"paid\",\"total\":9}}\n",
            "{\"error\":{\"code\":\"database_error\",\"message\":\"boom\"}}\n",
        ]))
        .collect()
        .await;
        assert_eq!(items.len(), 2);
        let err = items[1].as_ref().unwrap_err();
        assert_eq!(err.code(), "database_error");
        assert_eq!(err.status(), Some(503));
        assert_eq!(err.message(), "boom");
    }
}
