//! The rusqlite side of the SQLite suite: its backend, plus the bring-your-own checks that
//! drive a caller-owned rusqlite connection.

use based_runtime::rusqlite::Connection;
use serde_json::json;

use based_codegen::{sql, Dialect};
pub use based_runtime::RusqliteBackend as SqliteBackend;
use based_runtime::{AdoptedRusqlite, AdoptedTransport, Compiled, Engine, MockDb, SeqIdGen};

/// A fixture inserted through an adopted transaction commits with the caller's commit.
pub async fn adopted_fixture_inserts(compiled: Compiled) {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&sql::ddl(&compiled.schema, Dialect::Sqlite))
        .unwrap();
    let engine = Engine::new(
        compiled,
        SqliteBackend::in_memory().unwrap(),
        SeqIdGen::default(),
    );

    conn.execute_batch("BEGIN").unwrap();
    {
        let adopted = AdoptedTransport::new(engine.clone(), AdoptedRusqlite::new(&mut conn));
        let row = json!({ "name": "Adopted", "slug": "adopted", "tier": "free" });
        let response = adopted.insert_fixture("Org", row).await;
        assert_eq!(response.status, 200, "{:?}", response.body);
    }
    conn.execute_batch("COMMIT").unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM org", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

/// A raw caller write and baseddsl work on one caller-owned transaction commit together,
/// and roll back together when the caller does not commit.
pub async fn adopt_commits_atomically(c: Compiled) {
    let mut conn = Connection::open_in_memory().expect("caller connection");
    conn.execute_batch(&sql::ddl(&c.schema, Dialect::Sqlite))
        .expect("based DDL");
    conn.execute_batch(
        "INSERT INTO `widget` (`id`, `name`, `stock`) VALUES ('w1', 'Widget', 0);\
         CREATE TABLE audit (id TEXT PRIMARY KEY, widget_id TEXT NOT NULL, note TEXT NOT NULL);",
    )
    .expect("seed + app-owned audit table");

    let engine = Engine::new(c, MockDb::new(vec![]), SeqIdGen::default());

    // ---- commit path: raw audit write + baseddsl restock on one caller-owned tx ----------
    conn.execute_batch("BEGIN")
        .expect("caller begins its own tx");
    conn.execute(
        "INSERT INTO audit (id, widget_id, note) VALUES (?, ?, ?)",
        ("a1", "w1", "restocked to 50"),
    )
    .expect("raw audit insert");
    {
        let api = AdoptedTransport::new(engine.clone(), AdoptedRusqlite::new(&mut conn));
        let locked = api
            .dispatch("/q/widget_for_update", json!({ "id": "w1" }), json!({}))
            .await;
        assert_eq!(locked.status, 200, "adopted read: {:?}", locked.body);
        let wrote = api
            .dispatch("/m/restock", json!({ "id": "w1", "stock": 50 }), json!({}))
            .await;
        assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
        assert_eq!(wrote.body["stock"], 50);
    }
    conn.execute_batch("COMMIT")
        .expect("caller commits its own tx");
    assert_eq!(
        audit_and_stock(&conn),
        (50, 1),
        "both writes committed together"
    );

    // ---- rollback path: same two writes, caller rolls back -------------------------------
    conn.execute_batch("BEGIN")
        .expect("caller begins its own tx");
    conn.execute(
        "INSERT INTO audit (id, widget_id, note) VALUES (?, ?, ?)",
        ("a2", "w1", "discarded"),
    )
    .expect("raw audit insert");
    {
        let api = AdoptedTransport::new(engine.clone(), AdoptedRusqlite::new(&mut conn));
        let wrote = api
            .dispatch("/m/restock", json!({ "id": "w1", "stock": 999 }), json!({}))
            .await;
        assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
    }
    conn.execute_batch("ROLLBACK").expect("caller rolls back");
    assert_eq!(
        audit_and_stock(&conn),
        (50, 1),
        "both writes discarded together"
    );
}

fn audit_and_stock(conn: &Connection) -> (i64, i64) {
    let stock = conn
        .query_row("SELECT stock FROM `widget` WHERE id = 'w1'", [], |r| {
            r.get(0)
        })
        .expect("read stock");
    let audits = conn
        .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
        .expect("count audit");
    (stock, audits)
}
