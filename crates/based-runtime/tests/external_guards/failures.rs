use super::fixture::Fixture;
use super::harness::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn callback_failures_leave_no_rows_or_key_claims() {
    let oversized = "x".repeat(16 * 1024 + 1);
    let chunked = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n", oversized.len()).into_bytes();
    let cases = vec![
        (json_response(DENY), Duration::ZERO, "Permission denied"),
        (
            response("401 Unauthorized", "", "secret policy"),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (
            response("302 Found", "Location: /guard\r\n", ""),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (
            response("200 OK", "Content-Type: text/plain\r\n", ALLOW),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\nContent-Encoding: gzip\r\n",
                ALLOW,
            ),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (
            json_response(r#"{"version":1,"verdict":"allow","extra":true}"#),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (
            json_response(r#"{"version":1,"version":1,"verdict":"allow"}"#),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (
            json_response(&oversized),
            Duration::ZERO,
            "Guard check unavailable",
        ),
        (chunked, Duration::ZERO, "Guard check unavailable"),
        (Vec::new(), Duration::ZERO, "Guard check unavailable"),
        (
            json_response(ALLOW),
            Duration::from_millis(100),
            "Guard check unavailable",
        ),
    ];
    for (reply, delay, message) in cases {
        let fixture = Fixture::new().await;
        let mut callback = Callback::start(reply, delay).await;
        let result = fixture
            .create(
                &callback.guards("callback-secret", 40),
                json!({"name":"first"}),
            )
            .await;
        assert_eq!(result.status, 403, "{:?}", result.body);
        assert_eq!(result.body["error"]["code"], "guard_denied");
        assert_eq!(result.body["error"]["message"], message);
        assert_eq!(fixture.rows().await, json!([]));
        callback.received.recv().await.unwrap();
        *callback.reply.lock().unwrap() = json_response(ALLOW);
        let result = fixture
            .create(
                &callback.guards("callback-secret", 2000),
                json!({"name":"first"}),
            )
            .await;
        assert_eq!(
            result.status, 200,
            "denial must not consume key: {:?}",
            result.body
        );
        assert_eq!(fixture.rows().await.as_array().unwrap().len(), 1);
        callback.received.recv().await.unwrap();
        assert!(
            callback.received.try_recv().is_err(),
            "no redirect/retry invocations"
        );
    }
}

#[tokio::test]
async fn oversized_request_and_unreachable_callback_deny_without_writes() {
    let fixture = Fixture::new().await;
    let mut callback = Callback::start(json_response(ALLOW), Duration::ZERO).await;
    let guards = callback.guards("secret", 2000);
    let result = fixture
        .create(&guards, json!({"name":"x".repeat(256 * 1024)}))
        .await;
    assert_eq!(result.status, 403);
    assert!(
        callback.received.try_recv().is_err(),
        "oversized request must not be sent"
    );
    drop(callback);
    assert_eq!(
        fixture
            .create(&guards, json!({"name":"normal"}))
            .await
            .status,
        403
    );
    assert_eq!(fixture.rows().await, json!([]));
}

#[tokio::test]
async fn cancelling_dispatch_drops_callback_and_never_writes() {
    let fixture = Fixture::new().await;
    let mut callback = Callback::start(json_response(ALLOW), Duration::from_millis(300)).await;
    let guards = callback.guards("secret", 2000);
    {
        let call = fixture.create(&guards, json!({"name":"cancelled"}));
        tokio::pin!(call);
        tokio::select! {
            _ = callback.received.recv() => {},
            result = &mut call => panic!("callback returned early: {result:?}"),
        }
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(fixture.rows().await, json!([]));
    assert_eq!(
        fixture
            .create(&guards, json!({"name":"cancelled"}))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn forged_callback_credentials_are_rejected_without_consuming_key() {
    let fixture = Fixture::new().await;
    let callback = Callback::authenticated("correct-secret").await;
    assert_eq!(
        fixture
            .create(
                &callback.guards("forged-secret", 2000),
                json!({"name":"item"})
            )
            .await
            .status,
        403
    );
    assert_eq!(fixture.rows().await, json!([]));
    assert_eq!(
        fixture
            .create(
                &callback.guards("correct-secret", 2000),
                json!({"name":"item"})
            )
            .await
            .status,
        200
    );
}
