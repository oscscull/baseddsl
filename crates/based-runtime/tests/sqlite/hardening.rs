//! Pool hardening over the SQLite backends, mirroring the server backends' hardening suites:
//! a saturated pool fails a checkout fast as pool-exhausted rather than queueing.

use std::time::{Duration, Instant};

use based_runtime::{Backend, DbErrorKind, SqlitePoolConfig};

use crate::driver::SqliteBackend;

/// A file backend with a pool of one and a short checkout wait.
fn pool_of_one(tag: &str) -> SqliteBackend {
    let path = std::env::temp_dir().join(format!(
        "based-hardening-{}-{tag}.sqlite",
        std::process::id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let cfg = SqlitePoolConfig {
        max: 1,
        checkout_timeout: Duration::from_millis(200),
    };
    SqliteBackend::open_with_config(path.to_str().unwrap(), cfg).expect("open file sqlite")
}

/// With the only connection held, the next checkout waits at most `checkout_timeout` then
/// fails as [`DbErrorKind::PoolExhausted`] (the wire's 503), never an unbounded hang.
#[tokio::test]
async fn pool_exhaustion_fails_fast() {
    let backend = pool_of_one("checkout");
    let _held = backend
        .checkout("")
        .await
        .expect("first checkout holds the only connection");
    let start = Instant::now();
    let res = backend.checkout("").await;
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

/// A health probe against a saturated pool reports pool exhaustion, not an opaque failure.
#[tokio::test]
async fn ping_on_a_saturated_pool_is_pool_exhausted() {
    let backend = pool_of_one("ping");
    let _held = backend.checkout("").await.expect("hold the only connection");
    let err = backend.ping().await.expect_err("no free connection to ping on");
    assert_eq!(err.kind, DbErrorKind::PoolExhausted, "{}", err.message);
}
