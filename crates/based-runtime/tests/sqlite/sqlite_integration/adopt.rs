use super::*;

/// Bring-your-own transaction (`adopt`) against a real SQLite database (transactions.md rung
/// 3): a caller opens a transaction on its **own** `sqlx` pool, does a **raw non-baseddsl
/// write**, then runs baseddsl work (a `for update` read — a no-op lock on SQLite, but the
/// read still runs — and a mutation) through `AdoptedTransport` on that same transaction. The
/// raw write and the baseddsl write commit atomically, and roll back together when the caller
/// does not commit. `adopt` never begins/commits/rolls back — the caller owns the boundary.
#[tokio::test]
async fn adopt_commits_raw_and_baseddsl_writes_atomically_on_sqlite() {
    use based_runtime::{AdoptedSqlite, AdoptedTransport, Engine, MockDb, SeqIdGen};
    use sqlx::sqlite::SqlitePoolOptions;

    let c = compile_sqlite(
        r#"
        Widget { id: text, name: text, stock: int }
        shape WidgetRow from Widget { name, stock }
        query widget_for_update(id) -> WidgetRow {
            get Widget where (id = $id) for update;
        }
        mutation restock(id, stock) -> WidgetRow {
            update Widget where (id = $id) { stock = $stock }
        }
        "#,
    );

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
    let (stock, audits) = sqlite_audit_and_stock(&pool).await;
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
    let (stock, audits) = sqlite_audit_and_stock(&pool).await;
    assert_eq!(
        stock, 50,
        "rolled-back baseddsl write did not persist (still 50)"
    );
    assert_eq!(
        audits, 1,
        "rolled-back raw write did not persist (still just a1)"
    );
}

async fn sqlite_audit_and_stock(pool: &sqlx::SqlitePool) -> (i64, i64) {
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
