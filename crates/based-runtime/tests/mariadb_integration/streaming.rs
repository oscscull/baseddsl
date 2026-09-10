use super::*;

// ---- streaming reads over the live wire -------------------------------------

/// Start the real `based serve` listener over a live router on a free loopback port
/// and return its `host:port` — so a streaming test observes the actual NDJSON body a
/// deployed edge produces, not just the dispatch-level stream.
async fn serve_live(compiled: Compiled, backend: ShardRouter) -> String {
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

/// A `-> stream` query against live MariaDB, observed through the full wire: `200` +
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
            "INSERT INTO `item` (`id`, `name`, `rank`) VALUES \
                ('i1', 'a', 10), ('i2', 'b', 30), ('i3', 'c', 20);",
        )
        .await;

    let addr = serve_live(c, router).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/q/export_items"))
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
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON envelope"))
        .collect();
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

/// A to-many nested array rides back in **sort-cascade order** against a live MariaDB:
/// children seeded out of order come back ordered by the child model's `@sort`
/// (`comments`), and a relation `@sort` on the edge overrides the child model's own
/// (`pins`). Proves MariaDB's `JSON_ARRAYAGG(… ORDER BY …)` form executes for real.
#[tokio::test]
async fn nested_to_many_rows_ride_in_sort_cascade_order() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Ticket {
          id: text
          subject: text
          comments: Comment[]
          pins: Pin[] @sort(rank desc)
        }
        @sort(pos asc)
        Comment { id: text, ticket: Ticket, pos: int, body: text }
        @sort(rank asc)
        Pin { id: text, ticket: Ticket, rank: int, label: text }
        shape TicketDetail from Ticket { subject, comments { body }, pins { label } }
        query ticket_by_id(id) -> TicketDetail;
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO `ticket` (`id`, `subject`) VALUES ('t1', 'printer on fire');\n\
             INSERT INTO `comment` (`id`, `ticket_id`, `pos`, `body`) VALUES \
                ('c3', 't1', 3, 'third'), ('c1', 't1', 1, 'first'), ('c2', 't1', 2, 'second');\n\
             INSERT INTO `pin` (`id`, `ticket_id`, `rank`, `label`) VALUES \
                ('p1', 't1', 1, 'low'), ('p3', 't1', 3, 'top'), ('p2', 't1', 2, 'mid');",
        )
        .await;

    let resp = call(
        &c,
        &router,
        "POST",
        "/q/ticket_by_id",
        json!({ "id": "t1" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({
            "subject": "printer on fire",
            // child model `@sort(pos asc)` — the model tier of the cascade.
            "comments": [{ "body": "first" }, { "body": "second" }, { "body": "third" }],
            // relation `@sort(rank desc)` overrides Pin's model `@sort(rank asc)`.
            "pins": [{ "label": "top" }, { "label": "mid" }, { "label": "low" }]
        })
    );
}
