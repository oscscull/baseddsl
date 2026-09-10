use super::*;

#[tokio::test]
async fn enum_variant_filter_and_check_constraint_end_to_end() {
    // A self-contained enum schema: a `where status = <variant>` filter (lowered to a
    // string literal) executed live, plus proof the DB CHECK rejects a non-variant value.
    let c = compile_sqlite(
        r#"
        enum Status { pending, paid, shipped }
        Item {
          id: Id
          status: Status (default pending)
          name:   text
        }
        shape ItemRow from Item { status, name }
        query paid_items() -> ItemRow[] { list Item where (status = paid) order (name); }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .expect("generated DDL executes");
    backend
        .execute_batch(
            r#"
            INSERT INTO `item` (`id`, `status`, `name`) VALUES ('i1', 'paid', 'A');
            INSERT INTO `item` (`id`, `status`, `name`) VALUES ('i2', 'pending', 'B');
            INSERT INTO `item` (`id`, `status`, `name`) VALUES ('i3', 'paid', 'C');
            "#,
        )
        .await
        .expect("seed enum rows");

    // The variant filter runs live and returns only the two `paid` rows, with the enum
    // value round-tripping as its wire string.
    let resp = call(&c, &backend, "POST", "/q/paid_items", json!({}), json!({})).await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!([
            { "status": "paid", "name": "A" },
            { "status": "paid", "name": "C" }
        ])
    );

    // The generated CHECK constraint rejects a value outside the enum's variants.
    let bad = backend
        .execute_batch("INSERT INTO `item` (`id`, `status`, `name`) VALUES ('x', 'bogus', 'X');")
        .await;
    assert!(
        bad.is_err(),
        "DB should reject a non-variant enum value via the CHECK constraint"
    );
}

/// A host guard (auth.md Handle 3) that genuinely **reads the live database** before
/// deciding: closing an order is allowed while its row is still open; once the write
/// lands, the same call is denied because the guard's own SELECT sees the new state.
/// The whole pass — guard read, denial, and the guarded write — runs through the real
/// dispatch core against live SQLite.
#[tokio::test]
async fn guard_reads_the_live_database_before_the_write() {
    use based_runtime::{fetch_all, GuardVerdict, SqlValue};
    use std::sync::Arc;

    const SCHEMA: &str = r#"
        Order { id: Id, status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation close_order(id) -> OrderCard guard order_still_open {
            update Order where (id = $id) { status = "closed" };
        }
    "#;
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(!diags
        .iter()
        .any(|d| d.severity == based_diagnostics::Severity::Error && d.code != "E0260"));
    let c = Compiled::from_checked(schema, sf.decls, Dialect::Sqlite);

    let backend = Arc::new(SqliteBackend::in_memory().expect("open in-memory sqlite"));
    backend
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .expect("generated DDL");
    backend
        .execute_batch("INSERT INTO `order` (`id`, `status`, `total`) VALUES ('o-1', 'open', 9);")
        .await
        .expect("seed");

    // The guard owns its own resources: it captures the backend and runs its own
    // SELECT. It checks out before the mutation does, so the single pooled
    // connection is free during the read.
    let guard_backend = Arc::clone(&backend);
    let guards = Guards::new().register("order_still_open", move |req| {
        let backend = Arc::clone(&guard_backend);
        async move {
            let id = req.args["id"].as_str().unwrap_or_default().to_string();
            // Fail closed: a guard that cannot decide denies.
            let Ok(mut conn) = backend.checkout("").await else {
                return GuardVerdict::deny("cannot verify order state");
            };
            let rows = fetch_all(conn.fetch(
                "SELECT `status` FROM `order` WHERE `id` = ?",
                &[SqlValue::Text(id)],
            ))
            .await;
            match rows {
                Ok(rows)
                    if rows
                        .first()
                        .and_then(|r| r.get("status"))
                        .and_then(|v| v.as_str())
                        == Some("open") =>
                {
                    GuardVerdict::Allow
                }
                Ok(_) => GuardVerdict::deny("order is not open"),
                Err(_) => GuardVerdict::deny("cannot verify order state"),
            }
        }
    });

    let run = |args: serde_json::Value| {
        let c = &c;
        let backend = Arc::clone(&backend);
        let guards = &guards;
        async move {
            let ids = SeqIdGen::default();
            dispatch(
                c,
                &*backend,
                "",
                &ids,
                &NoStore,
                guards,
                None,
                "POST",
                "/m/close_order",
                args,
                json!({}),
                None,
            )
            .await
        }
    };

    // Open row → the guard's SELECT sees 'open' → allowed; the write lands and the
    // declared-shape re-select returns the closed row.
    let first = run(json!({ "id": "o-1" })).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "status": "closed", "total": 9 }));

    // Same call again: the guard's SELECT now sees 'closed' → denied, before any write.
    let second = run(json!({ "id": "o-1" })).await;
    assert_eq!(second.status, 403);
    assert_eq!(second.body["error"]["code"], "guard_denied");
    assert_eq!(second.body["error"]["message"], "order is not open");
}

/// Whole-query raw body live: the raw SELECT is the statement, `${param}` binds
/// positionally, `{table}` interpolates the target's table, and the rows decode
/// into the declared shape by column name. The raw text owns soft-delete — the
/// hand-written tombstone filter excludes the tombstoned row.
#[tokio::test]
async fn raw_query_body_end_to_end() {
    let c = compile_sqlite(
        r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, total: int }
        shape UserRow from User { name, total }
        query heavy_users(min: int) -> UserRow[] {
          raw`SELECT u.name AS name, u.total AS total
              FROM {table} u
              WHERE u.total >= ${min} AND u.deleted_at IS NULL
              ORDER BY u.total DESC`;
        }
        query top_user() -> UserRow {
          raw`SELECT name, total FROM user WHERE deleted_at IS NULL ORDER BY total DESC LIMIT 1`;
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
            "INSERT INTO `user` (`id`, `name`, `total`, `deleted_at`) VALUES
               ('a', 'Ada', 900, NULL),
               ('b', 'Bob', 500, NULL),
               ('c', 'Cud', 950, '2026-01-01T00:00:00Z'),
               ('d', 'Dee', 100, NULL);",
        )
        .await
        .expect("seed");

    // The bound `min` excludes Dee; the hand-written tombstone filter excludes Cud;
    // the raw ORDER BY holds.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/heavy_users",
        json!({ "min": 400 }),
        json!({}),
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    assert_eq!(
        got.body,
        json!([
            { "name": "Ada", "total": 900 },
            { "name": "Bob", "total": 500 }
        ])
    );

    // A scalar raw `get`: first row in the declared shape.
    let top = call(&c, &backend, "POST", "/q/top_user", json!({}), json!({})).await;
    assert_eq!(top.status, 200, "{:?}", top.body);
    assert_eq!(top.body, json!({ "name": "Ada", "total": 900 }));
}
