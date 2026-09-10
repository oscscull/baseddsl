use super::*;

// ---- streaming reads over the live wire -------------------------------------

/// Start the real `based serve` listener over a live router on a free loopback port
/// and return its `host:port` — so a streaming test observes the actual NDJSON body a
/// deployed edge produces, not just the dispatch-level stream.
async fn serve_live(compiled: Compiled, backend: PgRouter) -> String {
    use based_runtime::http::{serve_with_handle, ServeConfig, TrustedHeaderContext};
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let listen = addr.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        serve_with_handle(
            compiled,
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
    let _handle = rx.await.unwrap();
    addr
}

/// POST one streaming query and return its NDJSON body parsed line by line.
async fn stream_lines(addr: &str, route: &str) -> Vec<serde_json::Value> {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}{route}"))
        .json(&json!({}))
        .send()
        .await
        .expect("request runs");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()["content-type"].to_str().unwrap(),
        "application/x-ndjson"
    );
    let body = resp.text().await.expect("body reads");
    body.lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON envelope"))
        .collect()
}

/// A `-> stream` query against live Postgres, observed through the full wire: `200` +
/// `application/x-ndjson`, one `row` line per row in sort order, and the mandatory
/// terminal `done` whose count checksums the pass.
#[tokio::test]
async fn stream_query_delivers_ndjson_rows_and_done_live() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(rank desc)
        Item { id: text, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query export_items() -> stream ItemCard;
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO item (id, name, rank) VALUES \
                ('i1', 'a', 10), ('i2', 'b', 30), ('i3', 'c', 20);",
        )
        .await;

    let addr = serve_live(c, router).await;
    let lines = stream_lines(&addr, "/q/export_items").await;
    assert_eq!(
        lines,
        vec![
            json!({ "row": { "name": "b", "rank": 30 } }),
            json!({ "row": { "name": "c", "rank": 20 } }),
            json!({ "row": { "name": "a", "rank": 10 } }),
            json!({ "done": { "rows": 3 } }),
        ]
    );
}

/// The mid-stream failure gate, live: a raw-SQL shape field divides by a row value
/// that hits zero on the last row, so a genuine Postgres `division by zero` fires
/// **during** the pass — after rows have already gone out on the spent `200`. The
/// failure must arrive as the in-band terminal `error` line with the stable code,
/// and no `done` may follow.
#[tokio::test]
async fn stream_mid_pass_db_error_is_the_in_band_error_line_live() {
    let Some((c, router, container)) = live_schema(
        r#"
        Item { id: text, label: text, denom: int }
        shape ItemRow from Item { label, boom = raw`1 / denom` }
        query export_items() -> stream ItemRow;
        "#,
    )
    .await
    else {
        return;
    };
    // Physical order: two clean rows, then the poison row (denom = 0).
    container
        .exec_batch(
            "INSERT INTO item (id, label, denom) VALUES \
                ('i1', 'a', 1), ('i2', 'b', 1), ('i3', 'c', 0);",
        )
        .await;

    let addr = serve_live(c, router).await;
    let lines = stream_lines(&addr, "/q/export_items").await;

    // The terminal line is the in-band error — the status line was long spent.
    let last = lines.last().expect("the body carries a terminal line");
    assert_eq!(last["error"]["code"], "database_error", "lines: {lines:?}");
    assert!(
        last["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("division by zero")),
        "lines: {lines:?}"
    );
    // Everything before it is a delivered row — the failure interrupted a live pass.
    assert!(
        lines.len() >= 2,
        "rows must stream before the failure: {lines:?}"
    );
    for line in &lines[..lines.len() - 1] {
        assert!(line.get("row").is_some(), "lines: {lines:?}");
    }
}
