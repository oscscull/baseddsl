use super::*;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn request() -> GuardRequest {
    GuardRequest {
        callable: "make_item".into(),
        args: json!({"name":"sample"}),
        ctx: json!({"owner":"trusted"}),
        engine: None,
    }
}

async fn callback(
    response: Vec<u8>,
    delay: Duration,
) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let bytes = read_request(&mut stream).await;
        tokio::time::sleep(delay).await;
        let _ = stream.write_all(&response).await;
        bytes
    });
    (format!("http://{address}"), task)
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            if bytes.len() >= end + 4 + length {
                break;
            }
        }
        assert!(count > 0);
    }
    bytes
}

fn response(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn authenticated_allow_sends_only_protocol_fields() {
    let (url, task) = callback(
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"version":1,"verdict":"allow"}"#,
        ),
        Duration::ZERO,
    )
    .await;
    let guard = HttpGuard::new(&url, "private-token", Duration::from_secs(2), true).unwrap();
    assert_eq!(
        guard.check("permission", request()).await,
        GuardVerdict::Allow
    );
    let sent = String::from_utf8(task.await.unwrap()).unwrap();
    assert!(sent
        .to_lowercase()
        .contains("authorization: bearer private-token"));
    let body: serde_json::Value =
        serde_json::from_str(sent.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        body,
        json!({"version":1,"guard":"permission","callable":"make_item","args":{"name":"sample"},"ctx":{"owner":"trusted"}})
    );
}

#[tokio::test]
async fn invalid_transports_and_protocols_fail_closed() {
    let cases = [
        response("401 Unauthorized", "", "secret callback diagnostics"),
        response("302 Found", "Location: http://127.0.0.1:9\r\n", ""),
        response("200 OK", "Content-Type: text/plain\r\n", r#"{"version":1,"verdict":"allow"}"#),
        response("200 OK", "Content-Type: application/json\r\nContent-Encoding: gzip\r\n", "garbage"),
        response("200 OK", "Content-Type: application/json\r\n", r#"{"version":1,"verdict":"allow","unknown":true}"#),
        response("200 OK", "Content-Type: application/json\r\n", &" ".repeat(MAX_RESPONSE + 1)),
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n4001\r\n".iter().copied().chain(vec![b' '; MAX_RESPONSE + 1]).chain(b"\r\n0\r\n\r\n".iter().copied()).collect(),
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{}".to_vec(),
        vec![],
    ];
    for bytes in cases {
        let (url, task) = callback(bytes, Duration::ZERO).await;
        let guard = HttpGuard::new(&url, "secret", Duration::from_secs(2), true).unwrap();
        assert_eq!(
            guard.check("permission", request()).await,
            GuardVerdict::deny("Guard check unavailable")
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn denial_deadline_and_cancellation() {
    let (url, task) = callback(
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"version":1,"verdict":"deny","message":"Permission denied"}"#,
        ),
        Duration::ZERO,
    )
    .await;
    let guard = HttpGuard::new(&url, "secret", Duration::from_secs(2), true).unwrap();
    assert_eq!(
        guard.check("permission", request()).await,
        GuardVerdict::deny("Permission denied")
    );
    task.await.unwrap();
    let (url, task) = callback(vec![], Duration::from_secs(1)).await;
    let guard = HttpGuard::new(&url, "secret", Duration::from_millis(20), true).unwrap();
    assert_eq!(
        guard.check("permission", request()).await,
        GuardVerdict::deny("Guard check unavailable")
    );
    task.abort();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guard = HttpGuard::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "secret",
        Duration::from_secs(2),
        true,
    )
    .unwrap();
    // The registry retains its client while an individual invocation is cancelled.
    let _registered_guard = guard.clone();
    let check = tokio::spawn(async move { guard.check("permission", request()).await });
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut buffer = [0; 4096];
    assert!(!read_request(&mut socket).await.is_empty());
    check.abort();
    assert!(check.await.unwrap_err().is_cancelled());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), socket.read(&mut buffer))
            .await
            .unwrap()
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn oversize_request_never_reaches_callback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guard = HttpGuard::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "secret",
        Duration::from_secs(2),
        true,
    )
    .unwrap();
    let mut req = request();
    req.args = json!({"name":"x".repeat(MAX_REQUEST)});
    assert_eq!(
        guard.check("permission", req).await,
        GuardVerdict::deny("Guard check unavailable")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn startup_rejects_unsafe_configuration() {
    for url in [
        "http://127.0.0.1:123",
        "ftp://localhost",
        "https://secret@example.com",
        "https://example.com/#fragment",
        "invalid",
    ] {
        assert!(HttpGuard::new(url, "secret", Duration::from_secs(2), false).is_err());
    }
    assert!(HttpGuard::new("http://example.com", "secret", Duration::from_secs(2), true).is_err());
    for secret in ["", " ", "bad\nheader"] {
        assert!(
            HttpGuard::new("https://example.com", secret, Duration::from_secs(2), false).is_err()
        );
    }
    for duration in [Duration::ZERO, Duration::from_secs(31)] {
        assert!(HttpGuard::new("https://example.com", "secret", duration, false).is_err());
    }
}

#[tokio::test]
async fn internal_transaction_retry_does_not_resend_callback() {
    let (url, task) = callback(
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"version":1,"verdict":"allow"}"#,
        ),
        Duration::ZERO,
    )
    .await;
    let guard = HttpGuard::new(&url, "test", Duration::from_secs(2), true).unwrap();
    let file = based_parser::parse_file("Item { id: Id name: text } mutation make_item(name) -> Item guard permission { create Item { name = $name }; }", based_ast::FileId(0)).unwrap();
    let (schema, diagnostics) = based_sema::check(&file.decls);
    assert!(!diagnostics
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error));
    let compiled =
        crate::Compiled::from_checked(schema, file.decls, based_codegen::Dialect::Sqlite);
    let row = json!({"id":"item-1", "name":"sample"})
        .as_object()
        .unwrap()
        .clone();
    let db = crate::MockDb::new(vec![vec![row]]).deadlocking(1);
    let engine = crate::Engine::with_guards(
        compiled,
        db.clone(),
        crate::SeqIdGen::default(),
        guard.register(Guards::new(), "permission".into()),
    )
    .unwrap();
    let result = engine
        .call("/m/make_item", json!({"name":"sample"}), json!({}))
        .await;
    assert_eq!(result.status, 200, "{:?}", result.body);
    assert_eq!(db.tx_log(), vec!["begin", "rollback", "begin", "commit"]);
    task.await.unwrap();
}

#[tokio::test]
async fn redirect_target_is_never_contacted() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let headers = format!("Location: http://{}\r\n", target.local_addr().unwrap());
    let (url, task) = callback(
        response("307 Temporary Redirect", &headers, ""),
        Duration::ZERO,
    )
    .await;
    let guard = HttpGuard::new(&url, "test", Duration::from_secs(2), true).unwrap();
    assert_eq!(
        guard.check("permission", request()).await,
        GuardVerdict::deny("Guard check unavailable")
    );
    task.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), target.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn disconnect_is_not_automatically_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guard = HttpGuard::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "test",
        Duration::from_secs(2),
        true,
    )
    .unwrap();
    let task = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_request(&mut first).await;
        drop(first);
        // A forbidden retry would get a valid allow, making the assertion fail.
        if let Ok(Ok((mut second, _))) =
            tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
        {
            read_request(&mut second).await;
            second
                .write_all(&response(
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"{"version":1,"verdict":"allow"}"#,
                ))
                .await
                .unwrap();
            return true;
        }
        false
    });
    assert_eq!(
        guard.check("permission", request()).await,
        GuardVerdict::deny("Guard check unavailable")
    );
    assert!(!task.await.unwrap());
}
