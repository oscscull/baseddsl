use super::*;

/// The `serial` (DB-generated PK) read-back path against a live MariaDB server — the
/// `INSERT … RETURNING` path (MariaDB 11.4, D124): the INSERT omits the id, `AUTO_INCREMENT`
/// assigns it, and the RETURNING row is read back to key the declared-shape return.
#[tokio::test]
async fn serial_read_back_runs_against_live_mariadb() {
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

/// D124 against live MariaDB (`INSERT … RETURNING`): a bound create's `@created` timestamp
/// — engine-set, unknowable at plan time — is read back and reused by a sibling step. The
/// persisted `Event.at` must equal the Ticket's real `created_at`.
#[tokio::test]
async fn tx_binding_reuses_engine_timestamp_runs_against_live_mariadb() {
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

/// D124 against live MariaDB (retired E0268): a `serial` create bound `as o`, whose id the
/// DB assigns, binds a sibling FK via `$o.id` — the RETURNING read-back threads the
/// DB-generated id into the later step.
#[tokio::test]
async fn tx_binding_reaches_serial_id_runs_against_live_mariadb() {
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

/// A `@schema("…")`-qualified model lives in a named MariaDB *database*; a create + read-back
/// and a cross-database FK all run against the live server. Proves the qualifier reaches
/// INSERT, the read-back SELECT + JOIN, and the FK `REFERENCES core.org` constraint.
#[tokio::test]
async fn schema_qualified_models_create_read_and_fk_across_databases() {
    let Some(container) = MariaDbContainer::start().await else {
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
    let compiled = compile(src, Dialect::MariaDb);
    let router = ShardRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live MariaDB: {e:?}"));

    // Fresh, re-runnable namespaces (a MariaDB "schema" is a database).
    container
        .exec_batch(
            "DROP DATABASE IF EXISTS analytics; DROP DATABASE IF EXISTS core;\n\
             CREATE DATABASE core; CREATE DATABASE analytics;",
        )
        .await;
    container
        .exec_batch(&sql::ddl(&compiled.schema, Dialect::MariaDb))
        .await;
    container
        .exec_batch(&format!(
            "INSERT INTO `core`.`org` (`id`, `name`) VALUES ('{ORG_1}', 'Acme');"
        ))
        .await;

    // Create through the engine: INSERT INTO `analytics`.`event` with a FK into `core`.`org`,
    // then re-select in the declared shape (FROM `analytics`.`event` JOIN `core`.`org`).
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

/// `time` and `bytes` round-trip live against MariaDB: a `TIME` column binds/decodes via
/// chrono, a `BLOB` column base64-decodes at the bind and base64-encodes at the read. Proven
/// through a `create` (bind path) and a raw-seeded row (decode path independent of our binds).
#[tokio::test]
async fn time_and_bytes_columns_round_trip_live() {
    let Some((c, router, container)) = live_schema(
        r#"
        Event { id: Id, start_at: time, payload: bytes, label: text, @index(start_at) }
        shape EventCard from Event { start_at, payload, label }
        mutation add_event(start_at: time, payload: bytes, label: text) -> EventCard {
          create Event { start_at = $start_at, payload = $payload, label = $label };
        }
        query events() -> EventCard[] { list Event order (start_at asc); }
        "#,
    )
    .await
    else {
        return;
    };
    // Raw-seed a row (engine `Id` = a `CHAR(36)` uuid): a hex blob literal `x'000102FF'`
    // decodes to base64 `AAEC/w==` — proving the decode path independent of our own binds.
    container
        .exec_batch(
            "INSERT INTO `event` (`id`, `start_at`, `payload`, `label`) VALUES \
                ('00000000-0000-4000-8000-0000000000e0', '08:15:00', x'000102FF', 'seed');",
        )
        .await;

    // Create through the engine: `14:30:00` binds to a native `TIME`; `aGVsbG8=` ("hello")
    // base64-decodes into the `BLOB`. The read-back re-encodes both to the wire form.
    let created = call(
        &c,
        &router,
        "POST",
        "/m/add_event",
        json!({ "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" })
    );

    let all = call(&c, &router, "POST", "/q/events", json!({}), json!({})).await;
    assert_eq!(all.status, 200, "{:?}", all.body);
    assert_eq!(
        all.body,
        json!([
            { "start_at": "08:15:00", "payload": "AAEC/w==", "label": "seed" },
            { "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" }
        ])
    );
}

/// A composite `@key(device, seq)` with a DB-generated `serial` part (OP2), live against
/// MariaDB: two creates on the same device get `seq` 1 then 2 (`AUTO_INCREMENT`, keyed by
/// the covering `KEY (seq)` helper), the create response carries the DB-assigned `seq`
/// (recovered via `LAST_INSERT_ID()`, then the full tuple re-selected), and each row reads
/// back by its composite key.
#[tokio::test]
async fn composite_serial_key_part_is_db_generated_live_mariadb() {
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
            "INSERT INTO `device` (`id`, `name`) VALUES ('{device}', 'sensor');"
        ))
        .await;

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
