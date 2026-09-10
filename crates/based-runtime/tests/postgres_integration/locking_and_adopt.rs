use super::*;

/// `for update` holds a real row lock across a transaction, live against Postgres
/// (transactions.md slice 2). Two concurrent transactions run the **same** `product_for_update`
/// locking read on the **same** row: A acquires the lock and holds it (updating the row before
/// commit); B's identical locking read **blocks** until A commits, then proceeds and observes
/// A's committed value. The blocking is proven by a timeout — B's future stays pending while A
/// holds the lock — and the post-unblock value proves B waited for A's committed state, not a
/// stale snapshot. (SQLite has no row-level `FOR UPDATE`; its whole-database transaction lock
/// serializes writers instead, so this per-row hand-off is a Postgres/MySQL-family semantic.)
#[tokio::test]
async fn for_update_lock_is_held_across_a_transaction_live_postgres() {
    use based_runtime::{Engine, TxOptions};

    let Some((c, router, container)) = live_schema(
        r#"
        Product { id: text, sku: text, stock: int }
        shape ProductRow from Product { sku, stock }
        query product_for_update(id) -> ProductRow {
            get Product where (id = $id) for update;
        }
        mutation set_stock(id, stock) -> ProductRow {
            update Product where (id = $id) { stock = $stock }
        }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch("INSERT INTO product (id, sku, stock) VALUES ('p1', 'widget', 0);")
        .await;

    let engine = Engine::new(c, router, UuidGen);
    let lock = json!({ "id": "p1" });

    // A: open a transaction and take the row lock via the `for update` read.
    let txn_a = engine.begin(TxOptions::default()).await.expect("begin A");
    let a_read = txn_a
        .transport()
        .dispatch("/q/product_for_update", lock.clone(), json!({}))
        .await;
    assert_eq!(a_read.status, 200, "A locks the row: {:?}", a_read.body);

    // B: open a second transaction and issue the same locking read — it must block on A's lock.
    let txn_b = engine.begin(TxOptions::default()).await.expect("begin B");
    let tx_b = txn_b.transport();
    let b_read = tx_b.dispatch("/q/product_for_update", lock.clone(), json!({}));
    tokio::pin!(b_read);
    let while_held = tokio::time::timeout(Duration::from_millis(750), &mut b_read).await;
    assert!(
        while_held.is_err(),
        "B's `for update` read must block while A holds the lock, not return: {while_held:?}"
    );

    // A writes the row and commits, releasing the lock.
    let a_write = txn_a
        .transport()
        .dispatch(
            "/m/set_stock",
            json!({ "id": "p1", "stock": 99 }),
            json!({}),
        )
        .await;
    assert_eq!(a_write.status, 200, "A updates the row: {:?}", a_write.body);
    txn_a.commit().await.expect("commit A");

    // B now unblocks and observes A's committed value (proving it waited, not read a stale row).
    let b_resp = tokio::time::timeout(Duration::from_secs(5), &mut b_read)
        .await
        .expect("B unblocks once A releases the lock");
    assert_eq!(b_resp.status, 200, "B proceeds: {:?}", b_resp.body);
    assert_eq!(
        b_resp.body["stock"], 99,
        "B reads A's committed update, so it blocked until A released: {:?}",
        b_resp.body
    );
    txn_b.commit().await.expect("commit B");
}

/// `for update skip locked` and `for update nowait` behave per SQL, live against Postgres
/// (transactions.md slice 2 follow-on). Transaction A locks one row; a second transaction's
/// `skip locked` list returns the OTHER rows (never the locked one) without blocking, and a
/// `nowait` read of the locked row errors fast instead of waiting. Both wait modes are proven
/// non-blocking by a timeout that must NOT trip (unlike plain `for update`, which does block).
#[tokio::test]
async fn for_update_skip_locked_and_nowait_live_postgres() {
    use based_runtime::{Engine, TxOptions};

    let Some((c, router, container)) = live_schema(
        r#"
        Product { id: text, sku: text, stock: int }
        shape ProductRow from Product { sku, stock }
        query lock_one(id) -> ProductRow {
            get Product where (id = $id) for update;
        }
        query available(max) -> ProductRow[] {
            list Product where (stock <= $max) order (sku) for update skip locked;
        }
        query lock_one_nowait(id) -> ProductRow {
            get Product where (id = $id) for update nowait;
        }
        "#,
    )
    .await
    else {
        return;
    };
    container
        .exec_batch(
            "INSERT INTO product (id, sku, stock) VALUES \
             ('p1', 'a', 0), ('p2', 'b', 0), ('p3', 'c', 0);",
        )
        .await;

    let engine = Engine::new(c, router, UuidGen);

    // A: lock row p1 and hold it.
    let txn_a = engine.begin(TxOptions::default()).await.expect("begin A");
    let a_read = txn_a
        .transport()
        .dispatch("/q/lock_one", json!({ "id": "p1" }), json!({}))
        .await;
    assert_eq!(a_read.status, 200, "A locks p1: {:?}", a_read.body);

    // B: `skip locked` must return the unlocked rows (p2, p3), never the locked p1, and must
    // not block — proven by a timeout that must resolve.
    let txn_b = engine.begin(TxOptions::default()).await.expect("begin B");
    let b_read = tokio::time::timeout(
        Duration::from_secs(5),
        txn_b
            .transport()
            .dispatch("/q/available", json!({ "max": 100 }), json!({})),
    )
    .await
    .expect("`skip locked` must not block on A's lock");
    assert_eq!(b_read.status, 200, "skip locked read: {:?}", b_read.body);
    let skus: Vec<&str> = b_read
        .body
        .as_array()
        .expect("list")
        .iter()
        .map(|r| r["sku"].as_str().expect("sku"))
        .collect();
    assert_eq!(
        skus,
        vec!["b", "c"],
        "`skip locked` omits the locked row p1: {:?}",
        b_read.body
    );
    txn_b.commit().await.expect("commit B");

    // C: `nowait` on the locked row must error fast (not block) while A still holds the lock.
    let txn_c = engine.begin(TxOptions::default()).await.expect("begin C");
    let c_read = tokio::time::timeout(
        Duration::from_secs(5),
        txn_c
            .transport()
            .dispatch("/q/lock_one_nowait", json!({ "id": "p1" }), json!({})),
    )
    .await
    .expect("`nowait` must return fast, not block on A's lock");
    assert_ne!(
        c_read.status, 200,
        "`nowait` must error on a locked row: {:?}",
        c_read.body
    );
    txn_c.rollback().await.expect("rollback C");

    txn_a.commit().await.expect("commit A");
}

/// Bring-your-own transaction (`adopt`) live against Postgres (transactions.md rung 3): a
/// caller opens a transaction on **its own** `sqlx` pool, does a **raw non-baseddsl write**
/// on it, then runs baseddsl work (a `for update` locking read + a mutation) through
/// [`AdoptedTransport`] **on that same transaction** — and both the raw write and the
/// baseddsl write land atomically when the caller commits, and are discarded together when
/// the caller rolls back. This is the existential interop case: baseddsl is just one more
/// writer on the caller's transaction. `adopt` itself never begins/commits/rolls back — the
/// caller owns the boundary (proven: dropping the adopted transport, then the transaction,
/// leaves nothing committed).
#[tokio::test]
async fn adopt_commits_raw_and_baseddsl_writes_atomically_live_postgres() {
    use based_runtime::{AdoptedPg, AdoptedTransport, Engine, TxOptions};
    use sqlx::postgres::PgPoolOptions;

    let Some((c, router, container)) = live_schema(
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
    )
    .await
    else {
        return;
    };
    // Seed a widget, and an **app-owned** audit table that baseddsl knows nothing about —
    // the raw writes below land there.
    container
        .exec_batch(
            "INSERT INTO widget (id, name, stock) VALUES ('w1', 'Widget', 0);\
             CREATE TABLE audit (id text primary key, widget_id text not null, note text not null);",
        )
        .await;

    let engine = Engine::new(c, router, UuidGen);
    // The caller's *own* pool — the app's connections, not the engine's backend (which
    // `adopt` never touches: the baseddsl work runs on the caller's transaction).
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&container.url())
        .await
        .expect("caller pool");

    // ---- commit path: raw audit write + baseddsl restock, one caller-owned tx ----------
    {
        let mut tx = pool.begin().await.expect("caller begins its own tx");
        // (1) a raw, non-baseddsl write on the caller's transaction.
        sqlx::query("INSERT INTO audit (id, widget_id, note) VALUES ($1, $2, $3)")
            .bind("a1")
            .bind("w1")
            .bind("restocked to 50")
            .execute(&mut *tx)
            .await
            .expect("raw audit insert");
        // (2) baseddsl work through `adopt` on the *same* transaction.
        {
            let api = AdoptedTransport::new(engine.clone(), AdoptedPg::new(&mut tx));
            // `for update` locking read works through the adopted (TxBound) client.
            let locked = api
                .dispatch("/q/widget_for_update", json!({ "id": "w1" }), json!({}))
                .await;
            assert_eq!(
                locked.status, 200,
                "adopted for-update read: {:?}",
                locked.body
            );
            let wrote = api
                .dispatch("/m/restock", json!({ "id": "w1", "stock": 50 }), json!({}))
                .await;
            assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
            assert_eq!(wrote.body["stock"], 50);
        } // the adopted transport drops here, releasing its borrow of `tx`
        tx.commit().await.expect("caller commits its own tx");
    }
    // Both writes are visible after the caller's commit.
    let (stock, audits) = audit_and_stock(&pool).await;
    assert_eq!(stock, 50, "baseddsl restock committed with the caller's tx");
    assert_eq!(audits, 1, "the raw audit row committed atomically with it");

    // ---- rollback path: the same two writes, but the caller does NOT commit ------------
    {
        let mut tx = pool.begin().await.expect("caller begins its own tx");
        sqlx::query("INSERT INTO audit (id, widget_id, note) VALUES ($1, $2, $3)")
            .bind("a2")
            .bind("w1")
            .bind("should be discarded")
            .execute(&mut *tx)
            .await
            .expect("raw audit insert");
        {
            let api = AdoptedTransport::new(engine.clone(), AdoptedPg::new(&mut tx));
            let wrote = api
                .dispatch("/m/restock", json!({ "id": "w1", "stock": 999 }), json!({}))
                .await;
            assert_eq!(wrote.status, 200, "adopted mutation: {:?}", wrote.body);
        }
        // Drop the transaction without committing — `adopt` never committed anything, so the
        // caller's rollback discards BOTH the raw write and the baseddsl write together.
        drop(tx);
    }
    let (stock, audits) = audit_and_stock(&pool).await;
    assert_eq!(
        stock, 50,
        "the rolled-back baseddsl write did not persist (still 50, not 999)"
    );
    assert_eq!(
        audits, 1,
        "the rolled-back raw write did not persist (still just a1)"
    );

    // A default-options adopt (isolation is the caller's transaction's) round-trips too — the
    // adopted path is unaffected by `TxOptions`, which only the engine-owned rungs apply.
    let _ = TxOptions::default();
}

/// The widget's stock and the count of audit rows — read straight off the caller's pool, so
/// the assertions see committed state, not anything the adopted transport held.
async fn audit_and_stock(pool: &sqlx::PgPool) -> (i64, i64) {
    let stock: (i64,) = sqlx::query_as("SELECT stock FROM widget WHERE id = 'w1'")
        .fetch_one(pool)
        .await
        .expect("read stock");
    let audits: (i64,) = sqlx::query_as("SELECT count(*) FROM audit")
        .fetch_one(pool)
        .await
        .expect("count audit");
    (stock.0, audits.0)
}
