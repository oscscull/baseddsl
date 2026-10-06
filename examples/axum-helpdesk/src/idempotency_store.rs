//! Select the example's idempotency store at startup.

use based_runtime::{Compiled, DbStore, IdempotencyStore, PgRouter};
use std::time::Duration;

const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

pub async fn build(backend: PgRouter, compiled: &Compiled) -> Box<dyn IdempotencyStore> {
    if let Ok(url) = std::env::var("REDIS_URL") {
        let store = crate::redis_store::RedisStore::connect(&url, RETENTION)
            .await
            .expect("connect optional Redis replay store");
        println!("idempotency: optional Redis replay (fails open; no atomic SQL deduplication)");
        return Box::new(store);
    }
    let store = DbStore::with_gc(backend, compiled.dialect, RETENTION)
        .await
        .expect("initialize transactional idempotency store");
    println!("idempotency: transactional DbStore (24-hour retention, best-effort GC)");
    Box::new(store)
}
