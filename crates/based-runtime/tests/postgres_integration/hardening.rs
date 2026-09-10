use super::*;

// ---------- live-DB hardening ------------------------------

/// Bring up a live Postgres and build a router with the given [`PoolConfig`] — the seam for
/// the hardening tests, which each vary one knob (statement timeout, pool size, checkout
/// wait). Resets the schema so a persistent external server (`TEST_POSTGRES_URL`) is clean.
/// Returns `None` when Docker is unavailable (the caller skips).
async fn hardening(pool: PoolConfig) -> Option<(PgRouter, PostgresContainer)> {
    let container = PostgresContainer::start().await?;
    container.exec_batch(RESET_SQL).await;
    let router = PgRouter::single(&container.url(), pool)
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));
    Some((router, container))
}

/// A `statement_timeout` aborts a query that runs too long, live: the server cancels
/// `pg_sleep(5)` at the 500ms ceiling and the driver surfaces a `DbError` promptly, rather
/// than the connection hanging for the full sleep.
#[tokio::test]
async fn statement_timeout_aborts_a_long_query() {
    let pool = PoolConfig {
        statement_timeout: Duration::from_millis(500),
        ..PoolConfig::default()
    };
    let Some((router, _guard)) = hardening(pool).await else {
        return;
    };
    let mut db = router.checkout("").await.expect("checkout");
    let start = Instant::now();
    let res = fetch_all(db.fetch("SELECT pg_sleep(5)", &[])).await;
    let elapsed = start.elapsed();
    assert!(
        res.is_err(),
        "a query past statement_timeout must be aborted, not returned: {res:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "aborted at the timeout, not after the full 5s sleep: {elapsed:?}"
    );
}

/// A saturated pool fails fast as pool-exhausted, live: with a pool of one, a held
/// connection means the next checkout waits at most `checkout_timeout` then returns a
/// [`DbErrorKind::PoolExhausted`] `DbError` (the wire's 503) — never an unbounded hang.
#[tokio::test]
async fn pool_exhaustion_fails_fast() {
    let pool = PoolConfig {
        min: 1,
        max: 1,
        checkout_timeout: Duration::from_millis(500),
        statement_timeout: Duration::ZERO,
    };
    let Some((router, _guard)) = hardening(pool).await else {
        return;
    };
    let _held = router
        .checkout("")
        .await
        .expect("first checkout holds the only connection");
    let start = Instant::now();
    let res = router.checkout("").await;
    let elapsed = start.elapsed();
    match res {
        Err(e) => assert_eq!(e.kind, DbErrorKind::PoolExhausted, "{}", e.message),
        Ok(_) => panic!("a pool of one must not hand out a second connection while it is held"),
    }
    assert!(
        elapsed < Duration::from_secs(2),
        "failed fast at the checkout timeout, not a hang: {elapsed:?}"
    );
}

/// Two concurrent transactions that lock the same two rows in opposite order deadlock, live:
/// the server aborts exactly one side with a deadlock-class error (`40P01`) the driver
/// classifies as [`DbErrorKind::Deadlock`] (so the mutation path would retry it), and the
/// other commits. The barrier guarantees both hold their first lock before either reaches for
/// the second, so the deadlock is deterministic.
#[tokio::test]
async fn concurrent_transactions_surface_a_deadlock() {
    let Some((router, container)) = hardening(PoolConfig::default()).await else {
        return;
    };
    container
        .exec_batch(
            "CREATE TABLE acct (id text primary key, bal int);\n\
             INSERT INTO acct (id, bal) VALUES ('a', 0), ('b', 0);",
        )
        .await;
    let barrier = tokio::sync::Barrier::new(2);
    let (r1, r2) = tokio::join!(
        cross_lock(&router, "a", "b", &barrier),
        cross_lock(&router, "b", "a", &barrier),
    );
    let results = [r1, r2];
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(e) if e.kind == DbErrorKind::Deadlock)),
        "one side must be aborted with a deadlock-class error: {results:?}"
    );
    assert!(
        results.iter().any(std::result::Result::is_ok),
        "the other side must commit: {results:?}"
    );
}

/// One transaction of the crossed-lock deadlock: lock `first`, wait for the peer to lock its
/// own first row (the barrier), then reach for `second` — the loser is aborted (its `Tx`
/// drops uncommitted, which rolls back).
async fn cross_lock(
    router: &PgRouter,
    first: &str,
    second: &str,
    barrier: &tokio::sync::Barrier,
) -> Result<(), DbError> {
    let db: Box<dyn Db> = Box::new(router.checkout("").await?);
    let mut tx = db.begin().await?;
    tx.execute(
        &format!("UPDATE acct SET bal = bal + 1 WHERE id = '{first}'"),
        &[],
    )
    .await?;
    barrier.wait().await;
    tx.execute(
        &format!("UPDATE acct SET bal = bal + 1 WHERE id = '{second}'"),
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
