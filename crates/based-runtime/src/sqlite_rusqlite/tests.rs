use serde_json::json;

use super::*;
use crate::run::{fetch_all, Backend, DbErrorKind, DbRead};
use crate::value::SqlValue;

#[tokio::test]
async fn round_trips_a_write_then_read() {
    let backend = RusqliteBackend::in_memory().unwrap();
    let mut db = backend.checkout("").await.unwrap();
    db.execute(
        "CREATE TABLE t (id TEXT, n INTEGER, ok BOOLEAN, b BLOB, r REAL)",
        &[],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO t (id, n, ok, b, r) VALUES (?, ?, ?, ?, ?)",
        &[
            SqlValue::Text("a".into()),
            SqlValue::Int(5),
            SqlValue::Bool(true),
            SqlValue::Bytes("AAE=".into()),
            SqlValue::Float(1.5),
        ],
    )
    .await
    .unwrap();
    let rows = fetch_all(db.fetch(
        "SELECT id, n, ok, b, r FROM t WHERE id = ?",
        &[SqlValue::Text("a".into())],
    ))
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], json!("a"));
    assert_eq!(rows[0]["n"], json!(5));
    assert_eq!(rows[0]["ok"], json!(1));
    assert_eq!(rows[0]["b"], json!("AAE="));
    assert_eq!(rows[0]["r"], json!(1.5));
}

#[tokio::test]
async fn shared_connection_persists_across_checkouts() {
    let backend = RusqliteBackend::in_memory().unwrap();
    backend
        .execute_batch("CREATE TABLE t (id TEXT)")
        .await
        .unwrap();
    backend
        .checkout("")
        .await
        .unwrap()
        .execute("INSERT INTO t (id) VALUES ('x')", &[])
        .await
        .unwrap();
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
    assert_eq!(rows[0]["id"], json!("x"));
}

#[tokio::test]
async fn ping_ok_on_a_live_db() {
    assert!(RusqliteBackend::in_memory().unwrap().ping().await.is_ok());
}

#[tokio::test]
async fn typed_text_riding_binds_round_trip_verbatim() {
    let backend = RusqliteBackend::in_memory().unwrap();
    let mut db = backend.checkout("").await.unwrap();
    db.execute(
        "CREATE TABLE t (id TEXT, at TEXT, day TEXT, total TEXT)",
        &[],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO t VALUES (?, ?, ?, ?)",
        &[
            SqlValue::Uuid("00000000-0000-4000-8000-0000000000a1".into()),
            SqlValue::Timestamp("2024-01-02 12:30:45.500000".into()),
            SqlValue::Date("2024-01-02".into()),
            SqlValue::Decimal("19.90".into()),
        ],
    )
    .await
    .unwrap();
    let rows = fetch_all(db.fetch("SELECT * FROM t", &[])).await.unwrap();
    assert_eq!(rows[0]["id"], json!("00000000-0000-4000-8000-0000000000a1"));
    assert_eq!(rows[0]["at"], json!("2024-01-02 12:30:45.500000"));
    assert_eq!(rows[0]["day"], json!("2024-01-02"));
    assert_eq!(rows[0]["total"], json!("19.90"));
}

#[tokio::test]
async fn multi_statement_execute_spreads_params_and_sums_changes() {
    let backend = RusqliteBackend::in_memory().unwrap();
    backend
        .execute_batch("CREATE TABLE t (id TEXT)")
        .await
        .unwrap();
    let mut db = backend.checkout("").await.unwrap();
    let n = db
        .execute(
            "INSERT INTO t VALUES (?); INSERT INTO t VALUES (?);",
            &[SqlValue::Text("a".into()), SqlValue::Text("b".into())],
        )
        .await
        .unwrap();
    assert_eq!(n, 2);
    let rows = fetch_all(db.fetch("SELECT id FROM t ORDER BY id", &[]))
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            json!({ "id": "a" }).as_object().unwrap().clone(),
            json!({ "id": "b" }).as_object().unwrap().clone(),
        ]
    );
}

#[tokio::test]
async fn byo_connection_is_shared_with_the_caller() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (id TEXT); INSERT INTO t VALUES ('from-the-app');")
        .unwrap();
    let backend = RusqliteBackend::from_connection(conn);
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
    assert_eq!(rows[0]["id"], json!("from-the-app"));
    db.execute(
        "INSERT INTO t (id) VALUES (?)",
        &[SqlValue::Text("from-the-engine".into())],
    )
    .await
    .unwrap();
    drop(db);
    let n: i64 = backend
        .with_connection(|c| {
            c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
                .unwrap()
        })
        .await
        .unwrap();
    assert_eq!(n, 2, "the engine's write landed on the app's connection");
}

#[tokio::test]
async fn dropped_tx_rolls_back() {
    let backend = RusqliteBackend::in_memory().unwrap();
    backend
        .execute_batch("CREATE TABLE t (id TEXT)")
        .await
        .unwrap();
    let db = backend.checkout("").await.unwrap();
    let mut tx = db.begin().await.unwrap();
    tx.execute("INSERT INTO t (id) VALUES ('doomed')", &[])
        .await
        .unwrap();
    drop(tx);
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
    assert!(rows.is_empty(), "an uncommitted write must not survive");
}

#[tokio::test]
async fn committed_tx_persists() {
    let backend = RusqliteBackend::in_memory().unwrap();
    backend
        .execute_batch("CREATE TABLE t (id TEXT)")
        .await
        .unwrap();
    let db = backend.checkout("").await.unwrap();
    let mut tx = db.begin().await.unwrap();
    tx.execute("INSERT INTO t (id) VALUES ('kept')", &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn adopted_connection_runs_inside_the_callers_transaction() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (id TEXT); BEGIN;")
        .unwrap();
    {
        let mut adopted = AdoptedRusqlite::new(&mut conn);
        adopted
            .execute("INSERT INTO t VALUES (?)", &[SqlValue::Text("a".into())])
            .await
            .unwrap();
        let rows = fetch_all(adopted.fetch("SELECT id FROM t", &[]))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    }
    conn.execute_batch("ROLLBACK").unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "the caller's rollback discarded the adopted write");

    let err = AdoptedRusqlite::new(&mut conn)
        .execute("INSERT INTO t VALUES ('b')", &[])
        .await
        .unwrap_err();
    assert!(err.message.contains("no open transaction"), "{err}");
}

#[tokio::test]
async fn busy_is_a_retryable_deadlock() {
    let path = temp_db_path("busy");
    let backend = RusqliteBackend::open(&path).unwrap();
    backend
        .execute_batch("CREATE TABLE t (id TEXT)")
        .await
        .unwrap();
    let mut holder = rusqlite::Connection::open(&path).unwrap();
    holder.busy_timeout(std::time::Duration::ZERO).unwrap();
    let tx = holder
        .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
        .unwrap();
    let mut db = backend.checkout("").await.unwrap();
    db.execute("PRAGMA busy_timeout = 0", &[]).await.unwrap();
    let err = db
        .execute("INSERT INTO t VALUES ('x')", &[])
        .await
        .unwrap_err();
    assert_eq!(err.kind, DbErrorKind::Deadlock, "{err}");
    drop(tx);
}

fn temp_db_path(tag: &str) -> String {
    let mut p = std::env::temp_dir();
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("based_rusqlite_{tag}_{n}.db"));
    p.to_string_lossy().into_owned()
}

#[tokio::test]
async fn file_backend_opens_in_wal() {
    let backend = RusqliteBackend::open(&temp_db_path("wal")).unwrap();
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("PRAGMA journal_mode", &[]))
        .await
        .unwrap();
    assert_eq!(rows[0]["journal_mode"], json!("wal"));
}

#[tokio::test]
async fn file_backend_pool_exceeds_one() {
    let backend = RusqliteBackend::open(&temp_db_path("pool")).unwrap();
    backend
        .execute_batch("CREATE TABLE t (id TEXT); INSERT INTO t (id) VALUES ('x');")
        .await
        .unwrap();
    let mut a = backend.checkout("").await.unwrap();
    let mut b = backend.checkout("").await.unwrap();
    let ra = fetch_all(a.fetch("SELECT id FROM t", &[])).await.unwrap();
    let rb = fetch_all(b.fetch("SELECT id FROM t", &[])).await.unwrap();
    assert_eq!(ra[0]["id"], json!("x"));
    assert_eq!(rb[0]["id"], json!("x"));
}

#[test]
fn linked_sqlite_contains_the_upstream_wal_reset_fix() {
    assert!(
        rusqlite::version_number() >= 3_051_003,
        "linked SQLite {} predates the required 3.51.3 WAL-reset fix",
        rusqlite::version_number()
    );
}
