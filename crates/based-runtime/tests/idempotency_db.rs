//! Live proof of the DB-backed durable idempotency store (`DbStore`) against a real
//! SQLite database — the strong-form, atomic exactly-once guarantee.
//!
//! Two independent `SqliteBackend`s over the **same file database** stand in for two app
//! instances behind a load balancer. A keyed mutation runs on the first instance; the
//! *same* keyed retry then lands on the **second** instance and must replay the first's
//! response with **no** second write — dedupe travels through the shared `_based_idempotency`
//! table, not per-process memory. This is the property a `MemStore` cannot provide and the
//! reason a DB-first engine keeps its keys in the database.

#![cfg(feature = "sqlite")]

use serde_json::json;

use based_ast::FileId;
use based_codegen::{sql, Dialect};
use based_parser::parse_file;
use based_runtime::run::{fetch_all, Backend};
use based_runtime::{dispatch, Compiled, DbStore, Guards, SeqIdGen, SqliteBackend, WireResponse};
use based_sema::check;

const SCHEMA: &str = r#"
    Widget { id: Id, name: text }
    shape WidgetCard from Widget { name }
    mutation make_widget(name) -> WidgetCard {
        create Widget { name = $name };
    }
"#;

fn compiled() -> Compiled {
    let sf = parse_file(SCHEMA, FileId(0)).expect("parse");
    let (schema, diags) = check(&sf.decls);
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == based_diagnostics::Severity::Error),
        "schema errors: {diags:?}"
    );
    Compiled::from_checked(schema, sf.decls, Dialect::Sqlite)
}

/// Dispatch one keyed mutation against `backend` using the durable `store` — the exact
/// path `based serve` takes, minus the socket.
async fn make_widget(
    c: &Compiled,
    backend: &SqliteBackend,
    store: &DbStore,
    ids: &SeqIdGen,
    name: &str,
    key: &str,
) -> WireResponse {
    dispatch(
        c,
        backend,
        "",
        ids,
        store,
        &Guards::new(),
        None,
        "POST",
        "/m/make_widget",
        json!({ "name": name }),
        json!({}),
        Some(key.to_string()),
    )
    .await
}

async fn count(backend: &SqliteBackend, sql: &str) -> i64 {
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch(sql, &[])).await.unwrap();
    rows[0].values().next().unwrap().as_i64().unwrap()
}

#[tokio::test]
async fn keyed_retry_on_a_second_instance_dedupes_via_the_shared_table() {
    let c = compiled();
    let ids = SeqIdGen::default();

    // A file database both "instances" share (an in-memory DB is per-connection, so it
    // could not model two separate backends over one database).
    let path = std::env::temp_dir().join(format!(
        "based_idem_{}_{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let p = path.to_str().unwrap();
    let instance_a = SqliteBackend::open(p).unwrap();
    let instance_b = SqliteBackend::open(p).unwrap();

    // App schema, once, from the generated SQLite DDL.
    instance_a
        .execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .await
        .unwrap();

    // The durable store on each instance, both keyed to the same shared table.
    let store_a = DbStore::create(&instance_a, Dialect::Sqlite).await.unwrap();
    let store_b = DbStore::create(&instance_b, Dialect::Sqlite).await.unwrap();

    // First attempt lands on instance A.
    let first = make_widget(&c, &instance_a, &store_a, &ids, "alpha", "k1").await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "name": "alpha" }));
    assert_eq!(count(&instance_a, "SELECT COUNT(*) FROM `widget`").await, 1);

    // The SAME keyed retry lands on instance B: it must replay A's response, writing
    // nothing — exactly-once across instances, proven by the row count staying at 1.
    let replay = make_widget(&c, &instance_b, &store_b, &ids, "alpha", "k1").await;
    assert_eq!(replay.status, 200, "{:?}", replay.body);
    assert_eq!(replay.body, first.body, "the second instance replayed");
    assert_eq!(
        count(&instance_b, "SELECT COUNT(*) FROM `widget`").await,
        1,
        "no second write — the key deduped through the shared table"
    );
    assert_eq!(
        count(&instance_b, "SELECT COUNT(*) FROM `_based_idempotency`").await,
        1,
        "one key row, shared by both instances"
    );

    // The same key with a *different* payload is a reuse: reject (422), never replay the
    // wrong result, never write.
    let reused = make_widget(&c, &instance_b, &store_b, &ids, "different", "k1").await;
    assert_eq!(reused.status, 422, "{:?}", reused.body);
    assert_eq!(reused.body["error"]["code"], "idempotency_key_reuse");
    assert_eq!(count(&instance_b, "SELECT COUNT(*) FROM `widget`").await, 1);

    // A genuinely different key runs fresh.
    let fresh = make_widget(&c, &instance_b, &store_b, &ids, "beta", "k2").await;
    assert_eq!(fresh.status, 200, "{:?}", fresh.body);
    assert_eq!(fresh.body, json!({ "name": "beta" }));
    assert_eq!(count(&instance_a, "SELECT COUNT(*) FROM `widget`").await, 2);

    // Best-effort cleanup of the temp database + its journal.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{p}-journal"));
    let _ = std::fs::remove_file(format!("{p}-wal"));
    let _ = std::fs::remove_file(format!("{p}-shm"));
}
