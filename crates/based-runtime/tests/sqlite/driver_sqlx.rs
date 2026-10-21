//! The sqlx side of the SQLite suite: its backend, plus the bring-your-own checks that
//! drive a caller-owned sqlx transaction.

use serde_json::json;

use based_codegen::{sql, Dialect};
pub use based_runtime::SqliteBackend;
use based_runtime::{AdoptedSqlite, AdoptedTransport, Compiled, Engine, MockDb, SeqIdGen};
use sqlx::sqlite::SqlitePoolOptions;

/// A fixture inserted through an adopted transaction commits with the caller's commit.
pub async fn adopted_fixture_inserts(compiled: Compiled) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql::ddl(
        &compiled.schema,
        Dialect::Sqlite,
    )))
    .execute(&pool)
    .await
    .unwrap();
    let engine = Engine::new(
        compiled,
        SqliteBackend::from_pool(pool.clone()),
        SeqIdGen::default(),
    );

    let mut tx = pool.begin().await.unwrap();
    {
        let adopted = AdoptedTransport::new(engine.clone(), AdoptedSqlite::new(&mut tx));
        let row = json!({ "name": "Adopted", "slug": "adopted", "tier": "free" });
        let response = adopted.insert_fixture("Org", row).await;
        assert_eq!(response.status, 200, "{:?}", response.body);
    }
    tx.commit().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM org")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

/// A raw caller write and baseddsl work on one caller-owned transaction commit together,
/// and roll back together when the caller does not commit.
pub async fn adopt_commits_atomically(c: Compiled) {
    // The caller's own pool — pinned to one in-memory connection so the database persists
    // across checkouts. `adopt` never touches the engine's backend (a throwaway MockDb).
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("caller pool");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&pool)
        .await
        .expect("based DDL");
    sqlx::raw_sql(
        "INSERT INTO `widget` (`id`, `name`, `stock`) VALUES ('w1', 'Widget', 0);\
         CREATE TABLE audit (id TEXT PRIMARY KEY, widget_id TEXT NOT NULL, note TEXT NOT NULL);",
    )
    .execute(&pool)
    .await
    .expect("seed + app-owned audit table");

    let engine = Engine::new(c, MockDb::new(vec![]), SeqIdGen::default());

    // ---- commit path: raw audit write + baseddsl restock on one caller-owned tx ----------
    {
        let mut tx = pool.begin().await.expect("caller begins its own tx");
        sqlx::query("INSERT INTO audit (id, widget_id, note) VALUES (?, ?, ?)")
            .bind("a1")
            .bind("w1")
            .bind("restocked to 50")
            .execute(&mut *tx)
            .await
            .expect("raw audit insert");
        {
            let api = AdoptedTransport::new(engine.clone(), AdoptedSqlite::new(&mut tx));
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
        tx.commit().await.expect("caller commits its own tx");
    }
    let (stock, audits) = audit_and_stock(&pool).await;
    assert_eq!(stock, 50, "baseddsl restock committed with the caller's tx");
    assert_eq!(audits, 1, "the raw audit row committed atomically with it");

    // ---- rollback path: same two writes, caller does not commit --------------------------
    {
        let mut tx = pool.begin().await.expect("caller begins its own tx");
        sqlx::query("INSERT INTO audit (id, widget_id, note) VALUES (?, ?, ?)")
            .bind("a2")
            .bind("w1")
            .bind("discarded")
            .execute(&mut *tx)
            .await
            .expect("raw audit insert");
        {
            let api = AdoptedTransport::new(engine.clone(), AdoptedSqlite::new(&mut tx));
            let wrote = api
                .dispatch("/m/restock", json!({ "id": "w1", "stock": 999 }), json!({}))
                .await;
            assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
        }
        drop(tx); // no commit → both writes discarded together
    }
    let (stock, audits) = audit_and_stock(&pool).await;
    assert_eq!(
        stock, 50,
        "rolled-back baseddsl write did not persist (still 50)"
    );
    assert_eq!(
        audits, 1,
        "rolled-back raw write did not persist (still just a1)"
    );
}

async fn audit_and_stock(pool: &sqlx::SqlitePool) -> (i64, i64) {
    let stock: (i64,) = sqlx::query_as("SELECT stock FROM `widget` WHERE id = 'w1'")
        .fetch_one(pool)
        .await
        .expect("read stock");
    let audits: (i64,) = sqlx::query_as("SELECT count(*) FROM audit")
        .fetch_one(pool)
        .await
        .expect("count audit");
    (stock.0, audits.0)
}
