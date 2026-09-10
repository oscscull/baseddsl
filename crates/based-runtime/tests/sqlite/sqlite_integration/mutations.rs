use super::*;

/// An `update` mutation reads its row back in the **full declared shape**, not a bare
/// `{ id }`, keyed off the write's own `where` and run inside the same transaction
/// (read-your-writes) — proven against a real engine. The shape includes a nested to-one
/// sub-object (`placed_by { name }`), so the re-select exercises a relation join too.
#[tokio::test]
async fn update_mutation_reselects_full_declared_shape_end_to_end() {
    let c = compile_sqlite(
        r#"
        User { id: Id, name: text }
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, placed_by: User, status: text, total: int }
        shape OrderCard from Order { status, total, placed_by { name } }
        mutation set_status(id: Id, status: text) -> OrderCard {
          update Order where (id = $id) { status = $status };
        }
        query order_by_id(id) -> OrderCard;
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
            r#"
            INSERT INTO `user` (`id`, `name`) VALUES ('u1', 'Ada');
            INSERT INTO `order` (`id`, `updated_at`, `placed_by_id`, `status`, `total`)
                VALUES ('o1', '2020-01-01 00:00:00', 'u1', 'pending', 99);
            "#,
        )
        .await
        .expect("seed");

    // Update the status; the response is the *updated* row in its full declared shape
    // (new status, the unchanged total, and the nested buyer) — not `{ id }`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/set_status",
        json!({ "id": "o1", "status": "shipped" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "shipped", "total": 99, "placed_by": { "name": "Ada" } }),
        "read-your-writes: the re-select sees the new status under the same tx"
    );

    // The write committed: a fresh read sees the new status too.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({}),
    )
    .await;
    assert_eq!(
        got.body,
        json!({ "status": "shipped", "total": 99, "placed_by": { "name": "Ada" } })
    );
}

/// An atomic update expression (`total = total + $delta`) is computed **server-side** in
/// one statement, not read-modify-write — proven against a real engine: the read-your-writes
/// re-select shows the arithmetic result, and two sequential adjustments compose off the
/// stored value (no lost update).
#[tokio::test]
async fn atomic_update_expression_computes_server_side_end_to_end() {
    let c = compile_sqlite(
        r#"
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, status: text, total: int }
        shape OrderCard from Order { status, total }
        mutation adjust_total(id: Id, delta: int) -> OrderCard {
          update Order where (id = $id) { total = total + $delta };
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
            r#"
            INSERT INTO `order` (`id`, `updated_at`, `status`, `total`)
                VALUES ('o1', '2020-01-01 00:00:00', 'pending', 100);
            "#,
        )
        .await
        .expect("seed");

    // First adjustment: 100 + 25 = 125, computed in the database.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/adjust_total",
        json!({ "id": "o1", "delta": 25 }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(
        resp.body,
        json!({ "status": "pending", "total": 125 }),
        "read-your-writes: the re-select sees the server-computed sum"
    );

    // Second adjustment composes off the stored value: 125 - 5 = 120.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/adjust_total",
        json!({ "id": "o1", "delta": -5 }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "status": "pending", "total": 120 }));
}

/// A keyless (`@no_id`) legacy table inserts + reads back end to end: no `id` column
/// or `PRIMARY KEY` in the DDL, the INSERT sets no id, and the create's declared-shape
/// re-select keys on the `(unique)` column the create set (not a generated id). A `get`
/// keys on the same unique field.
#[tokio::test]
async fn keyless_model_inserts_and_reads_back_by_unique_end_to_end() {
    let c = compile_sqlite(
        r#"
        @no_id("append-only audit log keyed by its natural source, no surrogate id")
        AuditEvent { source: text (unique), action: text }
        shape EventRow from AuditEvent { source, action }
        query event_by_source(source) -> EventRow;
        mutation record_event(source: text, action: text) -> EventRow {
          create AuditEvent { source = $source, action = $action };
        }
        "#,
    );
    // The DDL is keyless: no `id` column, no PRIMARY KEY.
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    assert!(
        !ddl.contains("PRIMARY KEY"),
        "keyless table has no PK:\n{ddl}"
    );
    assert!(
        !ddl.contains("`id`"),
        "keyless table has no id column:\n{ddl}"
    );

    let backend = SqliteBackend::in_memory().expect("open sqlite");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    // Create: no generated id; the row reads back keyed on its unique `source`.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/m/record_event",
        json!({ "source": "svc-a", "action": "login" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "source": "svc-a", "action": "login" }));

    // Get by the unique key returns the same row.
    let resp = call(
        &c,
        &backend,
        "POST",
        "/q/event_by_source",
        json!({ "source": "svc-a" }),
        json!({}),
    )
    .await;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    assert_eq!(resp.body, json!({ "source": "svc-a", "action": "login" }));
}

/// An update whose `where` matches no row — a wrong id, or a cross-tenant id the scope
/// filter excludes — is a 404 `not_found` with nothing written, never a `200` with a
/// null body the typed client cannot decode.
#[tokio::test]
async fn zero_row_update_is_404_and_writes_nothing_end_to_end() {
    let c = compile_sqlite(
        r#"
        Org { id: Id, name: text }
        scope Tenant (org: Org = $ctx.org)
        @scope Tenant
        @updated(updated_at)
        Order { id: Id, updated_at: timestamp, org: Org, status: text }
        shape OrderCard from Order { status }
        mutation set_status(id: Id, status: text) -> OrderCard scoped Tenant {
          update Order where (id = $id) { status = $status };
        }
        query order_by_id(id) -> OrderCard scoped Tenant;
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
            r#"
            INSERT INTO `org` (`id`, `name`) VALUES ('org-a', 'A'), ('org-b', 'B');
            INSERT INTO `order` (`id`, `updated_at`, `org_id`, `status`)
                VALUES ('o1', '2020-01-01 00:00:00', 'org-a', 'pending');
            "#,
        )
        .await
        .expect("seed");

    // Another tenant names org-a's row: the scope filter makes the UPDATE match nothing.
    let cross = call(
        &c,
        &backend,
        "POST",
        "/m/set_status",
        json!({ "id": "o1", "status": "shipped" }),
        json!({ "org": "org-b" }),
    )
    .await;
    assert_eq!(cross.status, 404, "{:?}", cross.body);
    assert_eq!(cross.body["error"]["code"], "not_found");

    // A genuinely absent id under the owning tenant: same 404.
    let absent = call(
        &c,
        &backend,
        "POST",
        "/m/set_status",
        json!({ "id": "no-such", "status": "shipped" }),
        json!({ "org": "org-a" }),
    )
    .await;
    assert_eq!(absent.status, 404, "{:?}", absent.body);
    assert_eq!(absent.body["error"]["code"], "not_found");

    // Nothing was written: the owner still reads the original status.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({ "org": "org-a" }),
    )
    .await;
    assert_eq!(got.body, json!({ "status": "pending" }));
}
