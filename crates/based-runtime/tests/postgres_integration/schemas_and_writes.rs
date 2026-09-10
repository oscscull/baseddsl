use super::*;

/// A `@schema("…")`-qualified model lives in a named Postgres schema; a create + read-back
/// and a cross-schema FK all run against the live server. Proves the qualifier reaches
/// INSERT, the read-back SELECT + JOIN, and the FK `REFERENCES core.org` constraint.
#[tokio::test]
async fn schema_qualified_models_create_read_and_fk_across_namespaces() {
    let Some(container) = PostgresContainer::start().await else {
        return;
    };
    let src = r#"
        @schema("core")
        Org { id: Id, name: text }
        @schema("analytics")
        Event { id: Id, org: Org @fk, note: text }
        shape EventCard from Event { note, org { name } }
        query events() -> EventCard[];
        mutation add_event(org -> org, note) -> EventCard {
          create Event { org = $org, note = $note };
        }
        "#;
    let compiled = compile(src);
    let router = PgRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));

    container.exec_batch(RESET_SQL).await;
    container
        .exec_batch(
            "DROP SCHEMA IF EXISTS analytics CASCADE; DROP SCHEMA IF EXISTS core CASCADE;\n\
             CREATE SCHEMA core; CREATE SCHEMA analytics;",
        )
        .await;
    container
        .exec_batch(&sql::ddl(&compiled.schema, Dialect::Postgres))
        .await;
    container
        .exec_batch(&format!(
            "INSERT INTO \"core\".\"org\" (\"id\", \"name\") VALUES ('{ORG_1}', 'Acme');"
        ))
        .await;

    // Create the event through the engine: INSERT INTO "analytics"."event" with a FK into
    // "core"."org", then re-select in the declared shape (SELECT … FROM "analytics"."event"
    // JOIN "core"."org"). The nested `org { name }` proves the cross-schema JOIN resolved.
    let created = call(
        &compiled,
        &router,
        "POST",
        "/m/add_event",
        json!({ "org": ORG_1, "note": "hello" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "note": "hello", "org": { "name": "Acme" } })
    );

    // A second read path: the list query SELECTs FROM the qualified table + joins the
    // cross-schema parent.
    let listed = call(
        &compiled,
        &router,
        "POST",
        "/q/events",
        json!({}),
        json!({}),
    )
    .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(
        listed.body,
        json!([{ "note": "hello", "org": { "name": "Acme" } }])
    );
}

/// A composite `@key(device, seq)` with a DB-generated `serial` part (OP2), live against
/// Postgres: two creates on the same device get `seq` 1 then 2 (the `GENERATED … AS
/// IDENTITY` sequence), the create response carries the DB-assigned `seq` (recovered via
/// `RETURNING "seq"`, then the full tuple re-selected), and each row reads back by its
/// composite key.
#[tokio::test]
async fn composite_serial_key_part_is_db_generated_live_postgres() {
    let src = r#"
        Device { id: Id  name: text }
        @key(device, seq)
        Reading { device: Device  seq: serial  value: int }
        shape ReadingRow from Reading { seq, value }
        query reading(device, seq) -> ReadingRow;
        mutation record_reading(device -> device, value) -> ReadingRow {
          create Reading { device = $device, value = $value };
        }
        "#;
    let Some((c, router, container)) = live_schema(src).await else {
        return;
    };
    let device = "00000000-0000-4000-8000-0000000000d1";
    container
        .exec_batch(&format!(
            "INSERT INTO \"device\" (\"id\", \"name\") VALUES ('{device}', 'sensor');"
        ))
        .await;

    // Two readings on the same device: the DB assigns seq 1, then 2 — the create response
    // carries the generated value.
    let first = call(
        &c,
        &router,
        "POST",
        "/m/record_reading",
        json!({ "device": device, "value": 10 }),
        json!({}),
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "seq": 1, "value": 10 }));

    let second = call(
        &c,
        &router,
        "POST",
        "/m/record_reading",
        json!({ "device": device, "value": 20 }),
        json!({}),
    )
    .await;
    assert_eq!(second.status, 200, "{:?}", second.body);
    assert_eq!(second.body, json!({ "seq": 2, "value": 20 }));

    // Each row reads back by its composite key (device, seq) — the writes committed.
    let got = call(
        &c,
        &router,
        "POST",
        "/q/reading",
        json!({ "device": device, "seq": 2 }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(got.body, json!({ "seq": 2, "value": 20 }));
}

/// D124 against live Postgres (`INSERT … RETURNING`): a bound create's `@created` timestamp
/// — engine-set, unknowable at plan time — is read back and reused by a sibling step. The
/// persisted `Event.at` must equal the Ticket's real `created_at`.
#[tokio::test]
async fn tx_binding_reuses_engine_timestamp_runs_against_live_postgres() {
    const SCHEMA: &str = r#"
        @created(created_at)
        Ticket { id: Id, created_at: timestamp, subject: text }
        Event { id: Id, ticket: Ticket, at: timestamp, note: text }
        shape TicketRow from Ticket { subject, created_at }
        shape EventRow from Event { note, at, ticket = ticket.id }
        mutation open(subject: text, note: text) -> TicketRow {
          tx {
            create Ticket { subject = $subject } as t;
            create Event { ticket = $t.id, at = $t.created_at, note = $note };
          }
        }
        query events() -> EventRow[];
        query tickets() -> TicketRow[];
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let made = call(
        &c,
        &router,
        "POST",
        "/m/open",
        json!({ "subject": "S", "note": "N" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    let tickets = call(&c, &router, "POST", "/q/tickets", json!({}), json!({})).await;
    let events = call(&c, &router, "POST", "/q/events", json!({}), json!({})).await;
    let ticket_created = tickets.body[0]["created_at"]
        .as_str()
        .expect("ticket created_at");
    let event_at = events.body[0]["at"]
        .as_str()
        .unwrap_or_else(|| panic!("Event.at is NULL: {:?}", events.body[0]));
    assert_eq!(
        event_at, ticket_created,
        "$t.created_at must be the Ticket's real committed created_at"
    );
}

/// D124 against live Postgres (retired E0268): a `serial` create bound `as o`, whose id the
/// DB assigns, binds a sibling FK via `$o.id` — the RETURNING read-back threads the
/// DB-generated id into the later step.
#[tokio::test]
async fn tx_binding_reaches_serial_id_runs_against_live_postgres() {
    const SCHEMA: &str = r#"
        Org { id: serial, name: text }
        Note { id: Id, org: Org, body: text }
        shape OrgCard from Org { id, name }
        shape NoteRow from Note { body, org = org.id }
        mutation setup(name: text, body: text) -> OrgCard {
          tx {
            create Org { name = $name } as o;
            create Note { org = $o.id, body = $body };
          }
        }
        query notes() -> NoteRow[];
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let made = call(
        &c,
        &router,
        "POST",
        "/m/setup",
        json!({ "name": "Acme", "body": "hi" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);
    let org_id = made.body["id"].as_i64().expect("db-generated integer id");
    let notes = call(&c, &router, "POST", "/q/notes", json!({}), json!({})).await;
    assert_eq!(
        notes.body[0]["org"].as_i64(),
        Some(org_id),
        "Note.org must be the serial Org's DB-generated id: {:?}",
        notes.body[0]
    );
}

/// Nested writes end-to-end against live Postgres: a to-one child (customer), a to-many child
/// collection (items) with per-parent fan-out, and a DB-generated `serial` child whose FK is
/// learned from the INSERT via `RETURNING`.
#[tokio::test]
async fn nested_writes_run_against_live_postgres() {
    const SCHEMA: &str = r#"
        Customer { id: Id  name: text  email: text }
        LineItem { id: Id  order: Order  sku: text  qty: int }
        Order { id: Id  customer: Customer  total: int  items: LineItem[] }
        Author { id: serial  name: text }
        Book { id: Id  author: Author  title: text }

        shape OrderFull from Order { total, customer { name, email }, items { sku, qty } }
        shape LineIn from LineItem { sku, qty }
        shape BookIn from Book { title, author { name } }

        mutation place(rows: OrderFull[]) -> ok { create Order[] from $rows; }
        mutation add_books(rows: BookIn[]) -> ok { create Book[] from $rows; }
        query all_orders() -> OrderFull[];
        query all_lines() -> LineIn[];
        query all_books() -> BookIn[];
    "#;
    let Some((c, router, _guard)) = live_schema(SCHEMA).await else {
        return;
    };
    let rows = json!([
        { "total": 10, "customer": { "name": "Ann", "email": "ann@x.io" },
          "items": [ { "sku": "x1", "qty": 1 } ] },
        { "total": 20, "customer": { "name": "Bob", "email": "bob@x.io" },
          "items": [ { "sku": "y1", "qty": 2 }, { "sku": "y2", "qty": 3 } ] },
    ]);
    let r = call(
        &c,
        &router,
        "POST",
        "/m/place",
        json!({ "rows": rows }),
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200, "nested create failed: {:?}", r.body);

    let lines = call(&c, &router, "POST", "/q/all_lines", json!({}), json!({})).await;
    assert_eq!(
        lines.body.as_array().unwrap().len(),
        3,
        "3 line items across 2 orders"
    );

    let mut orders: Vec<serde_json::Value> =
        call(&c, &router, "POST", "/q/all_orders", json!({}), json!({}))
            .await
            .body
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(sort_items)
            .collect();
    orders.sort_by_key(|o| o["total"].as_i64().unwrap_or(0));
    assert_eq!(
        orders,
        json!([
            { "total": 10, "customer": { "name": "Ann", "email": "ann@x.io" },
              "items": [ { "sku": "x1", "qty": 1 } ] },
            { "total": 20, "customer": { "name": "Bob", "email": "bob@x.io" },
              "items": [ { "sku": "y1", "qty": 2 }, { "sku": "y2", "qty": 3 } ] },
        ])
        .as_array()
        .unwrap()
        .clone()
    );

    // A serial-id child: each book's author is created with a DB-generated id, linked via
    // RETURNING.
    let books = json!([
        { "title": "SICP",  "author": { "name": "Sussman" } },
        { "title": "TAOCP", "author": { "name": "Knuth" } },
    ]);
    let br = call(
        &c,
        &router,
        "POST",
        "/m/add_books",
        json!({ "rows": books }),
        json!({}),
    )
    .await;
    assert_eq!(br.status, 200, "serial nested create failed: {:?}", br.body);
    let mut got: Vec<serde_json::Value> =
        call(&c, &router, "POST", "/q/all_books", json!({}), json!({}))
            .await
            .body
            .as_array()
            .unwrap()
            .clone();
    got.sort_by_key(|b| b["title"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        got,
        json!([
            { "title": "SICP",  "author": { "name": "Sussman" } },
            { "title": "TAOCP", "author": { "name": "Knuth" } },
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}
