use super::*;

/// Keyset-cursor pagination, proven against a live Postgres — the Postgres twin of the
/// SQLite live keyset test. A `page (2)` keyset query walks the whole set exactly once: each full
/// page returns its window plus an opaque cursor, the final short page returns a `null` cursor,
/// and the cursor works even though the sort basis (`rank`, `id`) is not projected (the runtime
/// strips the hidden `__keyset_*` columns). A tampered cursor is a 400.
#[tokio::test]
async fn keyset_pagination_walks_the_set() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Item { id: text, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query items() -> ItemCard[] { list Item order (rank asc) page (2); }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO \"item\" (\"id\", \"name\", \"rank\") VALUES \
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30), \
                ('i4', 'd', 40), ('i5', 'e', 50);",
        )
        .await;

    let page = |args: serde_json::Value| call(&c, &router, "POST", "/q/items", args, json!({}));

    // Page 1 (no cursor): the two lowest-ranked rows + a "more" cursor (a full page).
    let p1 = page(json!({})).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(
        p1.body["rows"],
        json!([{ "name": "a", "rank": 10 }, { "name": "b", "rank": 20 }])
    );
    let c1 = p1.body["cursor"]
        .as_str()
        .expect("page 1 cursor")
        .to_string();

    // Page 2 (cursor from page 1): the next window, another full page → another cursor.
    let p2 = page(json!({ "cursor": c1 })).await;
    assert_eq!(
        p2.body["rows"],
        json!([{ "name": "c", "rank": 30 }, { "name": "d", "rank": 40 }])
    );
    let c2 = p2.body["cursor"]
        .as_str()
        .expect("page 2 cursor")
        .to_string();

    // Page 3 (cursor from page 2): the final row. A short page (1 < 2) → no more cursor.
    let p3 = page(json!({ "cursor": c2 })).await;
    assert_eq!(p3.body["rows"], json!([{ "name": "e", "rank": 50 }]));
    assert_eq!(p3.body["cursor"], json!(null), "last page has no cursor");

    // A tampered cursor is rejected at the boundary (400), never fed to the query.
    let bad = page(json!({ "cursor": "deadbeef.00" })).await;
    assert_eq!(bad.status, 400, "{:?}", bad.body);
    assert_eq!(bad.body["error"]["code"], json!("bad_cursor"));
}

/// Explicit offset pagination (`page (2) offset`), proven live against Postgres.
/// The client supplies an `offset`; the runtime binds it into `LIMIT … OFFSET …`. Paging
/// full→full→short walks the set, and an offset page envelope carries a `null` cursor (offset is
/// not keyset).
#[tokio::test]
async fn offset_pagination_pages_the_set() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Item { id: text, name: text, rank: int }
        shape ItemCard from Item { name, rank }
        query items() -> ItemCard[] { list Item order (rank asc) page (2) offset; }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO \"item\" (\"id\", \"name\", \"rank\") VALUES \
                ('i1', 'a', 10), ('i2', 'b', 20), ('i3', 'c', 30), \
                ('i4', 'd', 40), ('i5', 'e', 50);",
        )
        .await;

    let page = |args: serde_json::Value| call(&c, &router, "POST", "/q/items", args, json!({}));

    // Offset 0 (absent = first page): the first two rows, cursor null (offset is not keyset).
    let p1 = page(json!({})).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(
        p1.body["rows"],
        json!([{ "name": "a", "rank": 10 }, { "name": "b", "rank": 20 }])
    );
    assert_eq!(
        p1.body["cursor"],
        json!(null),
        "offset pages carry no cursor"
    );

    // Offset 2: the next window.
    let p2 = page(json!({ "offset": 2 })).await;
    assert_eq!(
        p2.body["rows"],
        json!([{ "name": "c", "rank": 30 }, { "name": "d", "rank": 40 }])
    );

    // Offset 4: the final short page.
    let p3 = page(json!({ "offset": 4 })).await;
    assert_eq!(p3.body["rows"], json!([{ "name": "e", "rank": 50 }]));
}

/// `uuid` + `timestamptz` result columns round-trip as canonical strings, and a keyset cursor
/// whose sort basis is a `timestamptz` (not just an int) walks the set. This is the regression
/// guard for the binary-format decode fix: Postgres results arrive in *binary* format, so a
/// `uuid` arrives as 16 raw bytes and a `timestamptz` as an i64 of microseconds — the decode
/// path turns both into their canonical string rather than mangling them (a raw text read
/// dropped the uuid hyphens and turned the timestamp into hex, which then failed to re-bind on
/// page 2).
#[tokio::test]
async fn uuid_and_timestamp_columns_round_trip_and_keyset() {
    let Some((c, router, container)) = live_schema(
        r#"
        @sort(id asc)
        Event { id: text, at: timestamp, label: text }
        shape EventCard from Event { id, at, label }
        query events() -> EventCard[] { list Event order (at asc) page (2); }
        "#,
    )
    .await
    else {
        return;
    };
    // `id: text` maps to TEXT here (plain string ids); `at` is a real `timestamptz`. Distinct,
    // ordered instants so the keyset basis is unambiguous.
    container
        .exec_batch(
            "INSERT INTO \"event\" (\"id\", \"at\", \"label\") VALUES \
                ('e1', '2024-01-01 00:00:00+00', 'a'), \
                ('e2', '2024-01-02 12:30:45.500000+00', 'b'), \
                ('e3', '2024-01-03 00:00:00+00', 'c');",
        )
        .await;

    let page = |args: serde_json::Value| call(&c, &router, "POST", "/q/events", args, json!({}));

    // Page 1: the two earliest events. The `timestamptz` comes back as a canonical ISO string
    // (decoded from binary microseconds), not hex — proving the fix on the projected column.
    let p1 = page(json!({})).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);
    assert_eq!(p1.body["rows"][0]["at"], json!("2024-01-01 00:00:00+00"));
    assert_eq!(
        p1.body["rows"][1]["at"],
        json!("2024-01-02 12:30:45.500000+00")
    );
    let cursor = p1.body["cursor"]
        .as_str()
        .expect("page 1 cursor")
        .to_string();

    // Page 2: feeding the cursor back binds the previous row's `timestamptz` basis — which only
    // works because the decoded string re-binds to the exact same instant (the bug's failure).
    let p2 = page(json!({ "cursor": cursor })).await;
    assert_eq!(p2.status, 200, "{:?}", p2.body);
    assert_eq!(
        p2.body["rows"],
        json!([{ "id": "e3", "at": "2024-01-03 00:00:00+00", "label": "c" }])
    );
    assert_eq!(p2.body["cursor"], json!(null), "last page has no cursor");
}

/// `time` and `bytes` round-trip live against Postgres — the risky path, since Postgres
/// transmits both binary: a `TIME` as an i64 of microseconds since midnight, a `BYTEA` as
/// raw bytes. The bind side parses the wire `HH:MM:SS` into a native `time` and base64-decodes
/// the wire string into `bytea`; the decode side turns the binary `time` back into its string
/// and base64-encodes the `bytea` — both through a `create` (bind) and a read-back (decode),
/// plus a raw-seeded row to prove the decoders independently of our own binds.
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
    // Raw-seed a row (engine `Id` = a real `uuid` column): a bytea hex literal `\x000102ff`
    // decodes to base64 `AAEC/w==`, and the `TIME` literal decodes (from binary microseconds)
    // back to `08:15:00` — proving the decoders independently of our own bind path.
    container
        .exec_batch(
            "INSERT INTO \"event\" (\"id\", \"start_at\", \"payload\", \"label\") VALUES \
                ('00000000-0000-4000-8000-0000000000e0', '08:15:00', '\\x000102ff', 'seed');",
        )
        .await;

    // Create through the engine: the bind parses `14:30:00` into a native `time` and base64-
    // decodes `aGVsbG8=` ("hello") into `bytea`; the read-back decodes both to the wire form.
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
        json!({ "start_at": "14:30:00", "payload": "aGVsbG8=", "label": "made" }),
        "create re-selects the time string + base64 bytes exact (binary bind + decode)"
    );

    // List both (ordered by time): the raw-seeded row's binary `TIME`/`BYTEA` decode to the
    // same canonical forms as the engine-bound row.
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

/// Soft-delete + restore read-back, proven live against Postgres. A soft
/// `delete` rewrites to `deleted_at = now()` (never a real DELETE) and reads the tombstoned row
/// back in its declared shape; the row then
/// vanishes from a live `list` (the soft-delete predicate is injected). `restore` clears the
/// tombstone and reads the row back with the live predicate applied — visible again.
#[tokio::test]
async fn soft_delete_and_restore_read_back() {
    let Some((c, router, container)) = live_schema(
        r#"
        @soft_delete(deleted_at)
        @sort(id asc)
        Widget { id: text, deleted_at: timestamp?, name: text }
        shape WidgetCard from Widget { name }
        query widgets() -> WidgetCard[] { list Widget; }
        mutation remove_widget(id: text) -> WidgetCard { delete Widget where (id = $id); }
        mutation restore_widget(id: text) -> WidgetCard { restore Widget where (id = $id); }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO \"widget\" (\"id\", \"name\") VALUES ('w1', 'Alpha'), ('w2', 'Beta');",
        )
        .await;

    let list = || call(&c, &router, "POST", "/q/widgets", json!({}), json!({}));

    // Both live to start.
    assert_eq!(
        list().await.body,
        json!([{ "name": "Alpha" }, { "name": "Beta" }])
    );

    // Soft delete w1: rewritten to a tombstone, read back in shape.
    let del = call(
        &c,
        &router,
        "POST",
        "/m/remove_widget",
        json!({ "id": "w1" }),
        json!({}),
    )
    .await;
    assert_eq!(del.status, 200, "{:?}", del.body);
    assert_eq!(del.body, json!({ "name": "Alpha" }));

    // The tombstone hides w1 from a live read (soft-delete predicate injected).
    assert_eq!(list().await.body, json!([{ "name": "Beta" }]));

    // Restore w1: tombstone cleared, read back live.
    let res = call(
        &c,
        &router,
        "POST",
        "/m/restore_widget",
        json!({ "id": "w1" }),
        json!({}),
    )
    .await;
    assert_eq!(res.status, 200, "{:?}", res.body);
    assert_eq!(res.body, json!({ "name": "Alpha" }));

    // w1 is visible again.
    assert_eq!(
        list().await.body,
        json!([{ "name": "Alpha" }, { "name": "Beta" }])
    );
}
