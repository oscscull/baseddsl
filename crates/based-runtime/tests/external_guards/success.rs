use super::fixture::Fixture;
use super::harness::*;
use based_runtime::http::{serve_with_guards, ServeConfig, TrustedHeaderContext};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn authenticated_allow_has_exact_body_and_replay_rechecks_permission() {
    let fixture = Fixture::new().await;
    let mut callback = Callback::start(json_response(ALLOW), Duration::ZERO).await;
    let guards = callback.guards("callback-secret", 2000);
    let first = fixture.create(&guards, json!({"name":"item"})).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    let request = callback.received.recv().await.unwrap();
    let end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
    assert!(headers.contains("authorization: bearer callback-secret"));
    assert!(!headers.contains("x-based-"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request[end + 4..]).unwrap(),
        json!({"version":1,"guard":"can_create","callable":"create_item","args":{"name":"item"},"ctx":{"user":"trusted-user"}})
    );
    assert_eq!(fixture.create(&guards, json!({"name":"item"})).await, first);
    *callback.reply.lock().unwrap() = json_response(DENY);
    assert_eq!(
        fixture.create(&guards, json!({"name":"item"})).await.status,
        403
    );
    assert_eq!(fixture.rows().await.as_array().unwrap().len(), 1);
    for _ in 0..2 {
        callback.received.recv().await.unwrap();
    }
    assert!(callback.received.try_recv().is_err());
}

#[tokio::test]
async fn listener_accepts_registered_callbacks_and_enforces_denial() {
    let fixture = Fixture::new().await;
    let callback = Callback::start(json_response(DENY), Duration::ZERO).await;
    let guards = callback.guards("secret", 2000);
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve_with_guards(
        fixture.compiled,
        fixture.backend,
        TrustedHeaderContext,
        Box::new(fixture.store),
        guards,
        ServeConfig {
            listen: address.to_string(),
        },
        move |handle| {
            assert!(tx.send(handle).is_ok());
        },
    ));
    let handle = rx.await.unwrap();
    let result = reqwest::Client::new()
        .post(format!("http://{address}/m/create_item"))
        .header("X-Based-Context", r#"{"user":"trusted-user"}"#)
        .json(&json!({"name":"denied"}))
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 403);
    let rows = reqwest::Client::new()
        .post(format!("http://{address}/q/items"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(rows, json!([]));
    handle.shutdown();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn internal_transaction_retry_does_not_resend_callback() {
    let fixture = Fixture::new().await;
    let mut callback = Callback::start(json_response(ALLOW), Duration::ZERO).await;
    let backend = based_runtime::MockDb::new(vec![vec![json!({"id":"item","name":"item"})
        .as_object()
        .unwrap()
        .clone()]])
    .deadlocking(1);
    let result = based_runtime::dispatch(
        &fixture.compiled,
        &backend,
        "",
        &based_runtime::SeqIdGen::default(),
        &based_runtime::NoStore,
        &callback.guards("secret", 2000),
        None,
        "POST",
        "/m/create_item",
        json!({"name":"item"}),
        json!({}),
        None,
    )
    .await;
    assert_eq!(result.status, 200, "{:?}", result.body);
    assert_eq!(
        backend.tx_log(),
        vec!["begin", "rollback", "begin", "commit"]
    );
    callback.received.recv().await.unwrap();
    assert!(callback.received.try_recv().is_err());
}
