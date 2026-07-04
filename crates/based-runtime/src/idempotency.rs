//! Write-retry idempotency (D25) — dedupe a retried `create`/mutation.
//!
//! App-side id-gen (D1) means the engine mints a *fresh* `id` for every `create`, so a
//! client that retries a mutation after a `503`/timeout — not knowing whether the first
//! attempt committed — would **double-insert** (D20 flagged this as the gap to close
//! before write retries are safe at scale). An *idempotency key* closes it: the caller
//! attaches a stable key to a mutation, and the engine runs the write body **at most
//! once** per key — a retry replays the first attempt's stored response instead of
//! writing again.
//!
//! ## Scope
//! - **Mutations only.** A query is naturally idempotent (no writes), so it never
//!   touches the store — only [`crate::run::run_mutation`] does.
//! - **Opt-in.** No key → the current behaviour (run every time). The key is *request
//!   metadata*, supplied out of band by the wire edge (`Idempotency-Key` header), never
//!   the JSON body — the same trusted-edge discipline as `$ctx` (auth.md/D7). A schema
//!   never *reads* the key: it is engine infrastructure, not application data, so it is
//!   deliberately **not** a `$ctx.<field>`.
//! - **Keyed by `(callable, key)`.** The key is scoped to the callable it accompanies, so
//!   the *same* key reused across two different mutations does not collide (a client that
//!   reuses one request id for a batch stays correct).
//!
//! ## Semantics (the Stripe/standard model)
//! On a mutation carrying a key, [`run_mutation`] consults the store via
//! [`IdempotencyStore::begin`]:
//! - **Fresh** → mark the key in-flight, run the write body, then [`record`] the response
//!   (or [`abandon`] on failure so a later retry may try again).
//! - **Done** → a prior attempt already committed; **replay** its stored response with no
//!   writes (the retry is a no-op, exactly-once achieved).
//! - **InFlight** → a concurrent attempt with the same key is still running; reject with a
//!   retryable `409` rather than run a second write (the double-submit case).
//!
//! ## The store is a seam
//! [`IdempotencyStore`] is a trait — the [`Db`](crate::run::Db)/[`IdGen`](crate::id::IdGen)
//! twin for this concern. [`MemStore`] is an in-process implementation (correct for a
//! single instance, and the whole request→response path is testable against it with no
//! infra). A production, multi-instance deployment backs the store with a shared store
//! (the database itself, or a cache) so a retry that lands on a *different* app instance
//! still dedupes — that durable backing is deferred (it needs live infra), but the seam
//! and the exactly-once logic land now.

use std::collections::HashMap;
use std::sync::Mutex;

/// What the store says about an idempotency key when a mutation asks to run under it.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyState {
    /// No prior attempt: the store has now marked the key in-flight and the caller
    /// should run the write body, then [`record`](IdempotencyStore::record) or
    /// [`abandon`](IdempotencyStore::abandon) it.
    Fresh,
    /// A prior attempt already completed: replay this stored response, run nothing.
    Done(serde_json::Value),
    /// A concurrent attempt with the same key is still running: do not run a second
    /// write — reject with a retryable conflict.
    InFlight,
}

/// A store that makes a keyed mutation run **at most once** per `(callable, key)`.
///
/// The three methods form the lifecycle: [`begin`](Self::begin) claims the key (or
/// reports it already done / in flight); on a claimed key the caller runs the write and
/// then [`record`](Self::record)s the response (success) or [`abandon`](Self::abandon)s
/// the claim (failure — a later retry may re-try). An implementation must make `begin`
/// atomic (claim-or-report) so two concurrent retries can never both run the write.
pub trait IdempotencyStore: Send + Sync {
    /// Atomically claim `(callable, key)` for this attempt, or report its existing
    /// state. On [`KeyState::Fresh`] the key is now marked in-flight (subsequent
    /// concurrent `begin`s see [`KeyState::InFlight`] until `record`/`abandon`).
    fn begin(&self, callable: &str, key: &str) -> KeyState;

    /// Record the successful response for a claimed key: future `begin`s replay it
    /// ([`KeyState::Done`]).
    fn record(&self, callable: &str, key: &str, response: serde_json::Value);

    /// Release a claimed key without recording a response (the attempt failed): a later
    /// retry may re-run the write. Called on the mutation-error path.
    fn abandon(&self, callable: &str, key: &str);
}

/// The state one key sits in inside a [`MemStore`].
enum Entry {
    /// A `begin` has claimed it; no response recorded yet.
    InFlight,
    /// A response has been recorded; `begin` replays it.
    Done(serde_json::Value),
}

/// An in-process [`IdempotencyStore`]: a `Mutex`-guarded map keyed by `(callable, key)`.
///
/// Correct for a **single** app instance (one process dedupes its own retries). It is
/// `Send + Sync`, so the shared HTTP worker pool (D20) uses one behind an `Arc`. A
/// multi-instance deployment wants a *shared* store (so a retry on another instance also
/// dedupes) — deferred to the driver/live-DB slice; the seam is identical. There is no
/// eviction yet (keys accumulate); a production store adds a TTL. For local/embedded use
/// and tests this is complete.
#[derive(Default)]
pub struct MemStore {
    entries: Mutex<HashMap<(String, String), Entry>>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }
}

impl IdempotencyStore for MemStore {
    fn begin(&self, callable: &str, key: &str) -> KeyState {
        let mut map = self.entries.lock().expect("idempotency store poisoned");
        let k = (callable.to_string(), key.to_string());
        match map.get(&k) {
            None => {
                map.insert(k, Entry::InFlight);
                KeyState::Fresh
            }
            Some(Entry::InFlight) => KeyState::InFlight,
            Some(Entry::Done(resp)) => KeyState::Done(resp.clone()),
        }
    }

    fn record(&self, callable: &str, key: &str, response: serde_json::Value) {
        let mut map = self.entries.lock().expect("idempotency store poisoned");
        map.insert(
            (callable.to_string(), key.to_string()),
            Entry::Done(response),
        );
    }

    fn abandon(&self, callable: &str, key: &str) {
        let mut map = self.entries.lock().expect("idempotency store poisoned");
        map.remove(&(callable.to_string(), key.to_string()));
    }
}

/// A no-op [`IdempotencyStore`] — every `begin` is [`KeyState::Fresh`] and nothing is
/// retained. This is the "idempotency off" store: dispatch paths that don't opt in (and
/// the tests that don't exercise dedupe) pass it so there is **one** dispatch code path
/// (principle 4), not a with/without-store fork. A [`crate::plan::Request`] with no key
/// also short-circuits the store entirely, so `NoStore` is only ever consulted for a
/// keyless request in practice.
#[derive(Default)]
pub struct NoStore;

impl IdempotencyStore for NoStore {
    fn begin(&self, _callable: &str, _key: &str) -> KeyState {
        KeyState::Fresh
    }
    fn record(&self, _callable: &str, _key: &str, _response: serde_json::Value) {}
    fn abandon(&self, _callable: &str, _key: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fresh_then_done_replays() {
        let s = MemStore::new();
        assert_eq!(s.begin("m", "k1"), KeyState::Fresh);
        // While in flight a concurrent begin is blocked.
        assert_eq!(s.begin("m", "k1"), KeyState::InFlight);
        s.record("m", "k1", json!({ "id": "a" }));
        // Once recorded, replay the stored response.
        assert_eq!(s.begin("m", "k1"), KeyState::Done(json!({ "id": "a" })));
    }

    #[test]
    fn abandon_frees_the_key_for_retry() {
        let s = MemStore::new();
        assert_eq!(s.begin("m", "k"), KeyState::Fresh);
        s.abandon("m", "k");
        // Abandoned → a retry sees it fresh again.
        assert_eq!(s.begin("m", "k"), KeyState::Fresh);
    }

    #[test]
    fn key_is_scoped_to_the_callable() {
        let s = MemStore::new();
        s.begin("m1", "shared");
        s.record("m1", "shared", json!(1));
        // The same key on a *different* mutation is independent.
        assert_eq!(s.begin("m2", "shared"), KeyState::Fresh);
    }

    #[test]
    fn no_store_is_always_fresh() {
        let s = NoStore;
        assert_eq!(s.begin("m", "k"), KeyState::Fresh);
        s.record("m", "k", json!(1));
        // Nothing retained: still fresh.
        assert_eq!(s.begin("m", "k"), KeyState::Fresh);
    }
}
