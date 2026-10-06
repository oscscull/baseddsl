//! The dedicated TLS gate supplies local server fixtures; ordinary infra-free runs skip.
#![cfg(all(feature = "tls-rustls", feature = "mariadb", feature = "postgres"))]

use based_runtime::driver::ShardRouter;
use based_runtime::run::{fetch_all, Backend};
use based_runtime::shard::PoolConfig;
use based_runtime::{DbRead, PgRouter};
use std::time::Duration;

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 2,
        checkout_timeout: Duration::from_secs(3),
        ..PoolConfig::default()
    }
}

async fn rejects(backend: impl Backend) {
    assert!(
        backend.checkout("").await.is_err(),
        "TLS verification must fail closed"
    );
}

#[tokio::test]
async fn postgres_verifies_certificate_and_hostname() {
    let Ok(url) = std::env::var("TEST_TLS_POSTGRES_URL") else {
        eprintln!("skip TLS fixture; run make ci-database-tls");
        return;
    };
    let router = PgRouter::single(&url, pool()).unwrap();
    let mut db = router
        .checkout("")
        .await
        .expect("verified Postgres connection");
    let rows = fetch_all(db.fetch(
        "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
        &[],
    ))
    .await
    .unwrap();
    assert_eq!(rows[0]["ssl"], serde_json::json!(true));
    rejects(PgRouter::single(&url.replace("@localhost:", "@127.0.0.1:"), pool()).unwrap()).await;
    let untrusted = url.replace("ca.crt", "untrusted.crt");
    rejects(PgRouter::single(&untrusted, pool()).unwrap()).await;
}

#[tokio::test]
async fn mariadb_verifies_certificate_and_hostname() {
    let Ok(url) = std::env::var("TEST_TLS_MARIADB_URL") else {
        eprintln!("skip TLS fixture; run make ci-database-tls");
        return;
    };
    let router = ShardRouter::single(&url, pool()).unwrap();
    let mut db = router
        .checkout("")
        .await
        .expect("verified MariaDB connection");
    let rows = fetch_all(db.fetch("SHOW SESSION STATUS LIKE 'Ssl_cipher'", &[]))
        .await
        .unwrap();
    assert!(!rows[0]["Value"].as_str().unwrap().is_empty());
    rejects(ShardRouter::single(&url.replace("@localhost:", "@127.0.0.1:"), pool()).unwrap()).await;
    let untrusted = url.replace("ca.crt", "untrusted.crt");
    rejects(ShardRouter::single(&untrusted, pool()).unwrap()).await;
}
