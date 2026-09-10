use super::*;

/// Idempotency (H6): a keyed mutation whose first attempt is a 404 (its `where` matched
/// no row) must **release** the claim — the transaction rolled back, nothing committed,
/// so a later retry with the same key must run fresh once the row exists, never replay a
/// phantom success and never wedge on a stranded in-flight claim. This exercises the
/// `MemStore` (out-of-band) `Fresh → NotFound → abandon` path end to end.
#[tokio::test]
async fn keyed_404_releases_the_claim_then_a_retry_runs_fresh() {
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
        .execute_batch(r#"INSERT INTO `org` (`id`, `name`) VALUES ('org-a', 'A');"#)
        .await
        .expect("seed org");

    let store = based_runtime::MemStore::new();
    let ids = SeqIdGen::default();
    let guards = Guards::new();
    let run = |args: serde_json::Value| {
        based_runtime::dispatch(
            &c,
            &backend,
            "",
            &ids,
            &store,
            &guards,
            None,
            "POST",
            "/m/set_status",
            args,
            json!({ "org": "org-a" }),
            Some("k1".to_string()),
        )
    };

    // First attempt: the order does not exist yet → 404, the tx rolled back, claim released.
    let first = run(json!({ "id": "o1", "status": "shipped" })).await;
    assert_eq!(first.status, 404, "{:?}", first.body);
    assert_eq!(first.body["error"]["code"], "not_found");

    // The row now exists (created out of band).
    backend
        .execute_batch(
            r#"INSERT INTO `order` (`id`, `updated_at`, `org_id`, `status`)
               VALUES ('o1', '2020-01-01 00:00:00', 'org-a', 'pending');"#,
        )
        .await
        .expect("insert order");

    // Retry with the *same* key + payload: the released claim lets it run fresh — a real
    // write (status → shipped), not a replay of the 404 and not a 409 in-flight conflict.
    let retry = run(json!({ "id": "o1", "status": "shipped" })).await;
    assert_eq!(
        retry.status, 200,
        "claim not released → wrong status: {:?}",
        retry.body
    );
    assert_eq!(retry.body, json!({ "status": "shipped" }));

    // The write actually persisted.
    let got = call(
        &c,
        &backend,
        "POST",
        "/q/order_by_id",
        json!({ "id": "o1" }),
        json!({ "org": "org-a" }),
    )
    .await;
    assert_eq!(got.body, json!({ "status": "shipped" }));
}

/// Soft-delete (H6): a soft `delete` mutation reads back the **tombstoned** row (the
/// re-select drops the live predicate), a later ordinary `get` misses it (the live
/// predicate excludes the tombstone), and a `restore` reads it back **live** again (the
/// re-select re-applies the live predicate to the now-live row). Proves the re-select's
/// `live` flag is correct for each verb against a real engine.
#[tokio::test]
async fn soft_delete_reads_back_tombstone_then_restore_reads_back_live() {
    let c = compile_sqlite(
        r#"
        @soft_delete(deleted_at)
        Note { id: Id, deleted_at: timestamp?, body: text }
        shape NoteCard from Note { body }
        mutation add_note(body: text) -> NoteCard { create Note { body = $body }; }
        mutation drop_note(id: Id) -> NoteCard { delete Note where (id = $id); }
        mutation undrop_note(id: Id) -> NoteCard { restore Note where (id = $id); }
        query note_by_id(id) -> NoteCard;
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    let made = call(
        &c,
        &backend,
        "POST",
        "/m/add_note",
        json!({ "body": "hi" }),
        json!({}),
    )
    .await;
    assert_eq!(made.status, 200, "{:?}", made.body);

    // The created row's id is the first minted id under a fresh SeqIdGen.
    let id = "id-0";

    // Soft delete: the tombstoned row still reads back (live predicate dropped for a delete).
    let dropped = call(
        &c,
        &backend,
        "POST",
        "/m/drop_note",
        json!({ "id": id }),
        json!({}),
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "tombstone must read back: {:?}",
        dropped.body
    );
    assert_eq!(dropped.body, json!({ "body": "hi" }));

    // An ordinary get now misses it (the live predicate excludes the tombstone).
    let gone = call(
        &c,
        &backend,
        "POST",
        "/q/note_by_id",
        json!({ "id": id }),
        json!({}),
    )
    .await;
    assert_eq!(
        gone.body,
        serde_json::Value::Null,
        "tombstone leaks into a live get: {:?}",
        gone.body
    );

    // Restore: the row is live again, and the re-select (live predicate re-applied) finds it.
    let back = call(
        &c,
        &backend,
        "POST",
        "/m/undrop_note",
        json!({ "id": id }),
        json!({}),
    )
    .await;
    assert_eq!(
        back.status, 200,
        "restore must read back the live row: {:?}",
        back.body
    );
    assert_eq!(back.body, json!({ "body": "hi" }));

    // And a live get sees it again.
    let seen = call(
        &c,
        &backend,
        "POST",
        "/q/note_by_id",
        json!({ "id": id }),
        json!({}),
    )
    .await;
    assert_eq!(seen.body, json!({ "body": "hi" }));
}
