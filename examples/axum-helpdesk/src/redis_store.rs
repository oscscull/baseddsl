//! A production idempotency store on Redis — the recommended store at scale.
//!
//! The engine dedupes keyed-mutation retries behind the public
//! [`IdempotencyStore`](based_runtime::IdempotencyStore) trait. The built-in options are an
//! in-process `MemStore` (per-instance; a retry that lands on another instance is not
//! deduped) and a `DbStore` (durable, but every key is another row in your database). This
//! example plugs in **Redis** instead: a shared, fast, out-of-band store whose **native key
//! expiry** bounds it (no sweep to run), so a keyed retry that lands on *any* instance
//! dedupes and old keys evict themselves.
//!
//! It lives **in the application**, not the engine: `based-runtime` carries no `redis`
//! dependency. Any store that satisfies the trait plugs in the same way — build it and pass
//! it to [`Engine::with_store`](based_runtime::Engine::with_store) (see `app.rs`).
//!
//! ## How the trait maps onto Redis
//! One Redis key per `(callable, idempotency-key)` holds a small JSON blob — the request
//! fingerprint plus, once known, the recorded response (`null` while the write is in
//! flight):
//! - [`begin`](RedisStore::begin) claims the key with a single atomic `SET … NX EX`. Set ⇒
//!   we are first ([`KeyState::Fresh`]). Already present ⇒ read it: a different fingerprint
//!   is one key reused for two requests ([`KeyState::Mismatch`]); an in-flight marker is a
//!   concurrent attempt ([`KeyState::InFlight`] → a retryable 409); a recorded response is
//!   an exactly-once [`KeyState::Done`] replay.
//! - [`record`](RedisStore::record) overwrites the marker with the response (same `EX`), so
//!   later attempts replay it.
//! - [`abandon`](RedisStore::abandon) deletes the key so a failed/cancelled attempt can be
//!   retried. It is the trait's one sync method (it is fired from the mutation's `Drop`
//!   guard, and `Drop` cannot `.await`), so it releases fire-and-forget on a spawned task.
//!
//! Redis being unreachable **fails open**: `begin` returns `Fresh`, so the write still runs
//! (dedupe degrades to at-least-once) rather than the desk rejecting every keyed write —
//! availability over exactly-once while the store is down.

use std::time::Duration;

use based_runtime::{Fingerprint, IdempotencyStore, KeyState};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, ExistenceCheck, SetExpiry, SetOptions};
use serde_json::json;

/// A [`ConnectionManager`]-backed idempotency store: a cheap, cloneable, auto-reconnecting
/// multiplexed handle to Redis, with a fixed key TTL (Redis expires keys itself, so there
/// is nothing to sweep).
pub struct RedisStore {
    conn: ConnectionManager,
    ttl_secs: u64,
}

impl RedisStore {
    /// Connect to Redis at `url` (e.g. `redis://127.0.0.1:6379`) and keep each key for
    /// `ttl` — the retry window a keyed mutation is deduped over.
    pub async fn connect(url: &str, ttl: Duration) -> redis::RedisResult<RedisStore> {
        let client = redis::Client::open(url)?;
        let conn = ConnectionManager::new(client).await?;
        Ok(RedisStore {
            conn,
            ttl_secs: ttl.as_secs().max(1),
        })
    }

    /// One namespaced Redis key per `(callable, idempotency-key)` — scoping the key to its
    /// callable, so the same key on two mutations never collides.
    fn redis_key(callable: &str, key: &str) -> String {
        format!("idem:{callable}:{key}")
    }

    /// Read a stored blob and classify it against this attempt's fingerprint.
    fn classify(blob: &str, fingerprint: Fingerprint) -> KeyState {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(blob) else {
            // Unparseable value: safest is to treat the key as claimed and make the caller
            // retry rather than run a possibly-duplicate write.
            return KeyState::InFlight;
        };
        let stored_fp = v.get("fp").and_then(|x| x.as_str()).unwrap_or_default();
        if stored_fp != fingerprint.to_string() {
            return KeyState::Mismatch;
        }
        match v.get("resp") {
            Some(resp) if !resp.is_null() => KeyState::Done(resp.clone()),
            _ => KeyState::InFlight,
        }
    }
}

#[async_trait::async_trait]
impl IdempotencyStore for RedisStore {
    async fn begin(&self, callable: &str, key: &str, fingerprint: Fingerprint) -> KeyState {
        let rk = Self::redis_key(callable, key);
        let mut conn = self.conn.clone();
        let inflight = json!({ "fp": fingerprint.to_string(), "resp": null }).to_string();

        // Atomic claim: SET rk <in-flight> NX EX ttl. "OK" ⇒ we claimed it fresh; nil ⇒ a
        // prior attempt holds it.
        let opts = SetOptions::default()
            .conditional_set(ExistenceCheck::NX)
            .with_expiration(SetExpiry::EX(self.ttl_secs));
        match conn
            .set_options::<_, _, Option<String>>(&rk, &inflight, opts)
            .await
        {
            Ok(Some(_)) => KeyState::Fresh,
            // Someone holds the key: read it and classify (done / in-flight / mismatch).
            Ok(None) => match conn.get::<_, Option<String>>(&rk).await {
                Ok(Some(blob)) => Self::classify(&blob, fingerprint),
                // Vanished between the SET and the GET (expired): make the caller retry
                // rather than risk a second write.
                Ok(None) => KeyState::InFlight,
                Err(_) => KeyState::Fresh, // fail open: run the write, dedupe degraded
            },
            Err(_) => KeyState::Fresh, // fail open: Redis unreachable, run the write
        }
    }

    async fn record(&self, callable: &str, key: &str, response: serde_json::Value) {
        let rk = Self::redis_key(callable, key);
        let mut conn = self.conn.clone();
        // Recover the fingerprint from the in-flight marker this attempt wrote — no other
        // writer touches a claimed key, so the GET reliably reads our own value.
        let fp = match conn.get::<_, Option<String>>(&rk).await {
            Ok(Some(blob)) => serde_json::from_str::<serde_json::Value>(&blob)
                .ok()
                .and_then(|v| v.get("fp").and_then(|x| x.as_str()).map(str::to_string)),
            _ => None,
        };
        // If the marker expired mid-write there is nothing to attach the response to; skip,
        // so a later retry re-runs rather than replaying under an unknown fingerprint.
        let Some(fp) = fp else {
            return;
        };
        let done = json!({ "fp": fp, "resp": response }).to_string();
        let opts = SetOptions::default().with_expiration(SetExpiry::EX(self.ttl_secs));
        let _ = conn.set_options::<_, _, ()>(&rk, done, opts).await;
    }

    fn abandon(&self, callable: &str, key: &str) {
        // Fired from the mutation's cancellation `Drop` guard, which cannot `.await`: release
        // the claim fire-and-forget on a spawned task, using our own cloned handle.
        let rk = Self::redis_key(callable, key);
        let mut conn = self.conn.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = conn.del::<_, ()>(&rk).await;
            });
        }
    }
}
