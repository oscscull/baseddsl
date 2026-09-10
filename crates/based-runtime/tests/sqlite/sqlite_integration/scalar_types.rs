use super::*;

/// End-to-end enum round-trip against a live engine: a string enum with a name≠value
/// variant (`paid = "PAID"`) and an int enum (`Priority`). A create assigns both by name;
/// the response shape carries their *wire* values (`"PAID"`, `2`); a filter on the string
/// enum and an *ordered* filter on the int enum each return the row; and the DB CHECK
/// rejects an out-of-range value directly inserted — proving the constraint is live.
#[tokio::test]
async fn enum_round_trip_string_and_int_end_to_end() {
    let c = compile_sqlite(
        r#"
        enum Status { pending, paid = "PAID", shipped }
        enum Priority { low = 0, medium = 1, high = 2 }
        Ticket { id: Id, status: Status (default pending), priority: Priority, title: text }
        shape TicketRow from Ticket { status, priority, title }
        mutation open_ticket(title: text) -> TicketRow {
          create Ticket { title = $title, priority = high, status = paid };
        }
        query urgent() -> TicketRow[] { list Ticket where (priority >= medium) order (title); }
        query by_paid() -> TicketRow[] { list Ticket where (status = paid) order (title); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    // Create: `status = paid` and `priority = high` are assigned by name; the response
    // carries their wire values ("PAID" for the renamed string variant, 2 for the int).
    let created = call(
        &c,
        &backend,
        "POST",
        "/m/open_ticket",
        json!({ "title": "server down" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "status": "PAID", "priority": 2, "title": "server down" }),
        "the shaped value carries the enum wire representation"
    );

    // Ordered filter on the int enum (`priority >= medium`) returns the row live.
    let urgent = call(&c, &backend, "POST", "/q/urgent", json!({}), json!({})).await;
    assert_eq!(urgent.status, 200, "{:?}", urgent.body);
    assert_eq!(
        urgent.body,
        json!([{ "status": "PAID", "priority": 2, "title": "server down" }])
    );

    // Filter on the string enum by name (`status = paid` → 'PAID') returns the row.
    let paid = call(&c, &backend, "POST", "/q/by_paid", json!({}), json!({})).await;
    assert_eq!(
        paid.body,
        json!([{ "status": "PAID", "priority": 2, "title": "server down" }])
    );

    // The DB CHECK rejects an out-of-range value inserted directly (defense in depth).
    let bad_int = backend.execute_batch(
        "INSERT INTO `ticket` (`id`, `status`, `priority`, `title`) VALUES ('x', 'PAID', 99, 't');",
    ).await;
    assert!(bad_int.is_err(), "int-enum CHECK must reject 99");
    let bad_str = backend.execute_batch(
        "INSERT INTO `ticket` (`id`, `status`, `priority`, `title`) VALUES ('y', 'bogus', 0, 't');",
    ).await;
    assert!(bad_str.is_err(), "string-enum CHECK must reject 'bogus'");
}

/// `in` value-list live: variants lower to their wire values, a `$param` element
/// binds at run time (its wire value on the wire), and rows outside the list are
/// excluded.
#[tokio::test]
async fn in_value_list_filters_end_to_end() {
    let c = compile_sqlite(
        r#"
        enum Status { pending, paid = "PAID", shipped }
        Ticket { id: Id, status: Status, title: text, @index(status) }
        shape TicketRow from Ticket { status, title }
        query active(extra: Status) -> TicketRow[] {
          list Ticket where (status in (pending, $extra)) order (title);
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    backend
        .execute_batch(
            "INSERT INTO `ticket` (`id`, `status`, `title`) VALUES
               ('a', 'pending', 'first'),
               ('b', 'PAID', 'second'),
               ('c', 'shipped', 'third');",
        )
        .await
        .expect("seed");

    // `$extra` carries the wire value ("PAID"); `shipped` is outside the list.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/active",
        json!({ "extra": "PAID" }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(
        got.body,
        json!([
            { "status": "pending", "title": "first" },
            { "status": "PAID", "title": "second" }
        ])
    );
}

/// `time` and `bytes` round-trip through a real SQLite database: a `time` rides the wire
/// as its `HH:MM:SS` string (stored TEXT — SQLite has no native TIME), a `bytes` value as
/// base64 (stored as a real `BLOB`, decoded/encoded at the driver boundary). An ordered
/// filter on the `time` column works (bytes stays equality-only, enforced by sema).
#[tokio::test]
async fn time_and_bytes_round_trip_end_to_end() {
    let c = compile_sqlite(
        r#"
        Event { id: Id, name: text, start_at: time, payload: bytes, @index(start_at) }
        shape EventRow from Event { name, start_at, payload }
        mutation add_event(name: text, start_at: time, payload: bytes) -> EventRow {
          create Event { name = $name, start_at = $start_at, payload = $payload };
        }
        query after(min: time) -> EventRow[] {
          list Event where (start_at >= $min) order (name);
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    // Raw-seed an earlier event: a SQLite blob literal `X'000102FF'` decodes back to base64
    // `AAEC/w==`, and the `08:00:00` time string round-trips (TEXT affinity).
    backend
        .execute_batch(
            "INSERT INTO `event` (`id`, `name`, `start_at`, `payload`) VALUES \
                ('seed', 'early', '08:00:00', X'000102FF');",
        )
        .await
        .expect("seed");

    // Create: a `time` is sent (and returned) as its `HH:MM:SS` string; a `bytes` value is
    // sent (and returned) as base64 ("hello" = `aGVsbG8=`) — the driver base64-decodes it into
    // the `BLOB` at the bind and re-encodes it on read-back.
    let created = call(
        &c,
        &backend,
        "POST",
        "/m/add_event",
        json!({ "name": "launch", "start_at": "14:30:00", "payload": "aGVsbG8=" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(
        created.body,
        json!({ "name": "launch", "start_at": "14:30:00", "payload": "aGVsbG8=" }),
        "create re-selects the row with the time string + base64 bytes exact"
    );

    // `start_at >= "09:00:00"` keeps only `launch` (08:00 < 09:00 lexicographically too),
    // proving the `time` column is ordered; the seeded blob decodes for the excluded row too.
    let filtered = call(
        &c,
        &backend,
        "POST",
        "/q/after",
        json!({ "min": "09:00:00" }),
        json!({}),
    )
    .await;
    assert_eq!(filtered.status, 200, "{:?}", filtered.body);
    assert_eq!(
        filtered.body,
        json!([{ "name": "launch", "start_at": "14:30:00", "payload": "aGVsbG8=" }])
    );
}

#[tokio::test]
async fn decimal_and_float_round_trip_end_to_end() {
    let c = compile_sqlite(
        r#"
        Ledger { id: Id, name: text, price: decimal(12, 2), score: float }
        shape LedgerRow from Ledger { name, price, score }
        mutation add_entry(name: text, price: decimal(12, 2), score: float) -> LedgerRow {
          create Ledger { name = $name, price = $price, score = $score };
        }
        query pricey(min: decimal(12, 2) >= price) -> LedgerRow[]
          unindexed(unsafe) order (name);
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));
    // A seeded row whose exact decimal (with a trailing zero) must survive the read.
    backend
        .execute_batch(
            "INSERT INTO `ledger` (`id`, `name`, `price`, `score`) VALUES ('seed', 'cheap', '0.10', 0.25);",
        ).await
        .expect("seed");

    // Create: a decimal is sent (and returned) as its exact string, a float as a number.
    let big = call(
        &c,
        &backend,
        "POST",
        "/m/add_entry",
        json!({ "name": "pricey", "price": "19.99", "score": 1.5 }),
        json!({}),
    )
    .await;
    assert_eq!(big.status, 200, "{:?}", big.body);
    assert_eq!(
        big.body,
        json!({ "name": "pricey", "price": "19.99", "score": 1.5 }),
        "create re-selects the row with the decimal exact + float as a number"
    );

    // An ordered comparison (`price >= 10.00`) filters correctly; the seeded `0.10` is
    // excluded, the created `19.99` kept — and both read back byte-exact.
    let filtered = call(
        &c,
        &backend,
        "POST",
        "/q/pricey",
        json!({ "min": "10.00" }),
        json!({}),
    )
    .await;
    assert_eq!(filtered.status, 200, "{:?}", filtered.body);
    assert_eq!(
        filtered.body,
        json!([{ "name": "pricey", "price": "19.99", "score": 1.5 }])
    );
}
