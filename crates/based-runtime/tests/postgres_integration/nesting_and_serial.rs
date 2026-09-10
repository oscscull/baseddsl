use super::*;

/// A to-many nested array rides back in **sort-cascade order** against a live Postgres:
/// children seeded out of order come back ordered by the child model's `@sort`
/// (`comments`), and a relation `@sort` on the edge overrides the child model's own
/// (`pins`). Proves Postgres's `json_agg(… ORDER BY …)` form executes for real.
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
            "INSERT INTO ticket (id, subject) VALUES ('t1', 'printer on fire');\n\
             INSERT INTO comment (id, ticket_id, pos, body) VALUES \
                ('c3', 't1', 3, 'third'), ('c1', 't1', 1, 'first'), ('c2', 't1', 2, 'second');\n\
             INSERT INTO pin (id, ticket_id, rank, label) VALUES \
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

/// The `serial` (DB-generated PK) read-back path against a live Postgres server: the
/// INSERT omits the id, Postgres assigns it via `GENERATED ALWAYS AS IDENTITY`, and the
/// engine reads it back with `RETURNING id` to key the declared-shape return.
#[tokio::test]
async fn serial_read_back_runs_against_live_postgres() {
    const SCHEMA: &str = r#"
        Org { id: serial  name: text }
        shape OrgCard from Org { id, name }
        mutation create_org(name) -> OrgCard { create Org { name = $name }; }
        query org_by_id(id) -> OrgCard;
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let first = call(
        &c,
        &router,
        "POST",
        "/m/create_org",
        json!({ "name": "Acme" }),
        json!({}),
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    let id1 = first.body["id"].as_i64().expect("db-generated integer id");
    assert_eq!(first.body["name"], json!("Acme"));

    let second = call(
        &c,
        &router,
        "POST",
        "/m/create_org",
        json!({ "name": "Globex" }),
        json!({}),
    )
    .await;
    let id2 = second.body["id"].as_i64().expect("id");
    assert!(id2 > id1, "serial ids increment: {id1} then {id2}");

    let got = call(
        &c,
        &router,
        "POST",
        "/q/org_by_id",
        json!({ "id": id1 }),
        json!({}),
    )
    .await;
    assert_eq!(got.body, json!({ "id": id1, "name": "Acme" }));
}
