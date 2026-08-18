//! Write-retry idempotency — dedupe a retried `create`/mutation.
//!
//! The engine mints a fresh `id` for every `create`, so a client that retries a mutation
//! after a `503`/timeout — not knowing whether the first attempt committed — would
//! double-insert. An idempotency key closes it: the caller attaches a stable key to a
//! mutation, and the engine runs the write body at most once per key — a retry replays
//! the first attempt's stored response instead of writing again.
//!
//! ## Scope
//! - Mutations only. A query is naturally idempotent (no writes), so it never touches the
//!   store — only [`crate::run::run_mutation`] does.
//! - Opt-in. No key → run every time. The key is request metadata, supplied out of band
//!   by the wire edge (`Idempotency-Key` header), never the JSON body. A schema never
//!   reads the key: it is engine infrastructure, not application data.
//! - Keyed by `(callable, key)`. The key is scoped to the callable it accompanies, so the
//!   same key reused across two different mutations does not collide.
//!
//! ## Semantics
//! On a mutation carrying a key, [`run_mutation`] consults the store via
//! [`IdempotencyStore::begin`], which also carries a request fingerprint (a stable hash of
//! the request's args + `$ctx`, [`Request::fingerprint`](crate::Request::fingerprint)):
//! - Fresh → mark the key in-flight (recording the fingerprint), run the write body, then
//!   [`record`] the response (or [`abandon`] on failure so a later retry may try again).
//! - Done → a prior attempt with the same fingerprint already committed; replay its stored
//!   response with no writes (exactly-once).
//! - InFlight → a concurrent attempt with the same key + fingerprint is still running;
//!   reject with a retryable `409` rather than run a second write.
//! - Mismatch → the key was seen before but with a different fingerprint: the caller
//!   reused one key for two different requests. Replaying the first would silently return
//!   the wrong request's result, so this is rejected (a non-retryable `422`) rather than
//!   run or replayed.
//!
//! ## The store is a seam
//! [`IdempotencyStore`] is a trait. [`MemStore`] is an in-process implementation (correct
//! for a single instance, and the whole request→response path is testable against it with
//! no infra). A multi-instance deployment backs the store with a shared/durable store (the
//! database itself, or a cache) so a retry that lands on a different app instance still
//! dedupes, behind the same trait, and plugs its own store in without editing runtime
//! source: [`Engine::with_store`](crate::Engine::with_store) for an embed, or
//! [`serve_with_store`](crate::http::serve_with_store) for the standalone listener.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a [`MemStore`] key is retained before it expires. A retried mutation arrives
/// within seconds of the first attempt, so a day is ample for dedupe while bounding how
/// long a key occupies memory (the industry-standard idempotency-key window).
const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// A stable hash of a request's args + `$ctx` — the payload a keyed mutation carries.
///
/// Two attempts that are genuine retries of the same request produce the same
/// fingerprint; a caller that accidentally reuses one key for two different requests
/// produces different ones, which the store rejects rather than silently replaying the
/// first. Built by [`Request::fingerprint`](crate::Request::fingerprint); opaque and
/// compared only for equality (the exact hash is never surfaced).
pub type Fingerprint = u64;

/// What the store says about an idempotency key when a mutation asks to run under it.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyState {
    /// No prior attempt: the store has now marked the key in-flight (with this attempt's
    /// fingerprint) and the caller should run the write body, then
    /// [`record`](IdempotencyStore::record) or [`abandon`](IdempotencyStore::abandon) it.
    Fresh,
    /// A prior attempt with the **same fingerprint** already completed: replay this stored
    /// response, run nothing.
    Done(serde_json::Value),
    /// A concurrent attempt with the same key + fingerprint is still running: do not run a
    /// second write — reject with a retryable conflict.
    InFlight,
    /// The key was seen before but with a **different** fingerprint (the caller reused one
    /// key for two different requests). Neither run nor replay — reject loudly, since
    /// replaying the first attempt's response would answer the wrong request.
    Mismatch,
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
    ///
    /// `fingerprint` is a stable hash of this attempt's request payload (args + `$ctx`,
    /// [`Request::fingerprint`](crate::Request::fingerprint)): a claimed/completed key
    /// replays/blocks only for a *matching* fingerprint, and a mismatch — the same key on a
    /// **different** request — is [`KeyState::Mismatch`] (reject, don't replay the wrong
    /// result). An implementation must make `begin` atomic (claim-or-report) so two
    /// concurrent retries can never both run the write.
    fn begin(&self, callable: &str, key: &str, fingerprint: Fingerprint) -> KeyState;

    /// Record the successful response for a claimed key: future `begin`s replay it
    /// ([`KeyState::Done`]).
    fn record(&self, callable: &str, key: &str, response: serde_json::Value);

    /// Release a claimed key without recording a response (the attempt failed): a later
    /// retry may re-run the write. Called on the mutation-error path.
    fn abandon(&self, callable: &str, key: &str);
}

/// One key's stored state inside a [`MemStore`]: its [`Fingerprint`] (so a later `begin`
/// under a *different* fingerprint is caught as a [`KeyState::Mismatch`] rather than
/// replayed/blocked), its outcome, and the instant it expires.
struct Entry {
    fingerprint: Fingerprint,
    outcome: Outcome,
    expires_at: Instant,
}

/// Whether a claimed key is still running or has a recorded response to replay.
enum Outcome {
    /// A `begin` has claimed it; no response recorded yet.
    InFlight,
    /// A response has been recorded; `begin` replays it for a matching fingerprint.
    Done(serde_json::Value),
}

/// The [`MemStore`] map plus the next time a full sweep of expired keys is due.
struct State {
    entries: HashMap<(String, String), Entry>,
    next_sweep: Instant,
}

impl State {
    /// Drop every expired key when a sweep interval has elapsed, so keys never accumulate
    /// past their TTL even if never revisited. Amortized: one pass per `ttl`, not per call.
    fn sweep_if_due(&mut self, now: Instant, ttl: Duration) {
        if now >= self.next_sweep {
            self.entries.retain(|_, e| e.expires_at > now);
            self.next_sweep = now + ttl;
        }
    }
}

/// An in-process [`IdempotencyStore`]: a `Mutex`-guarded map keyed by `(callable, key)`,
/// with per-key TTL expiry.
///
/// Correct for a single app instance (one process dedupes its own retries). It is
/// `Send + Sync`, so the shared HTTP worker pool uses one behind an `Arc`. A
/// multi-instance deployment wants a shared store (so a retry on another instance also
/// dedupes) behind the same trait — injected via
/// [`Engine::with_store`](crate::Engine::with_store) /
/// [`serve_with_store`](crate::http::serve_with_store). Keys expire after the TTL
/// ([`DEFAULT_TTL`], or [`with_ttl`](Self::with_ttl)): an expired key reads as fresh, and
/// a periodic sweep reclaims keys that are never revisited, so memory stays bounded on a
/// long-lived server.
pub struct MemStore {
    ttl: Duration,
    state: Mutex<State>,
}

impl MemStore {
    /// A store with the default 24-hour key TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// A store whose keys expire after `ttl`. A retry must arrive within `ttl` of the
    /// first attempt to be deduped; after it the key reads as fresh and is reclaimed.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl,
            state: Mutex::new(State {
                entries: HashMap::new(),
                next_sweep: Instant::now() + ttl,
            }),
        }
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.state
            .lock()
            .expect("idempotency store poisoned")
            .entries
            .len()
    }
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IdempotencyStore for MemStore {
    fn begin(&self, callable: &str, key: &str, fingerprint: Fingerprint) -> KeyState {
        let now = Instant::now();
        let mut st = self.state.lock().expect("idempotency store poisoned");
        st.sweep_if_due(now, self.ttl);
        let k = (callable.to_string(), key.to_string());
        // A key seen before only replays/blocks for the *same* request payload and while
        // unexpired; a different fingerprint is one key reused for two requests → reject.
        let live = st
            .entries
            .get(&k)
            .filter(|e| e.expires_at > now)
            .map(|e| match &e.outcome {
                _ if e.fingerprint != fingerprint => KeyState::Mismatch,
                Outcome::InFlight => KeyState::InFlight,
                Outcome::Done(resp) => KeyState::Done(resp.clone()),
            });
        live.unwrap_or_else(|| {
            // Absent or expired → claim it fresh (overwriting any expired entry).
            st.entries.insert(
                k,
                Entry {
                    fingerprint,
                    outcome: Outcome::InFlight,
                    expires_at: now + self.ttl,
                },
            );
            KeyState::Fresh
        })
    }

    fn record(&self, callable: &str, key: &str, response: serde_json::Value) {
        let now = Instant::now();
        let mut st = self.state.lock().expect("idempotency store poisoned");
        let k = (callable.to_string(), key.to_string());
        // Preserve the fingerprint the `begin` claim recorded (a `record` always follows a
        // `Fresh` claim for the same request). If the claim is somehow gone, fall back to a
        // fingerprint that never matches a future `begin`, so a stray record can't be
        // replayed under a mismatched payload.
        let fingerprint = st
            .entries
            .get(&k)
            .map_or(Fingerprint::MAX, |e| e.fingerprint);
        st.entries.insert(
            k,
            Entry {
                fingerprint,
                outcome: Outcome::Done(response),
                expires_at: now + self.ttl,
            },
        );
    }

    fn abandon(&self, callable: &str, key: &str) {
        let mut st = self.state.lock().expect("idempotency store poisoned");
        st.entries.remove(&(callable.to_string(), key.to_string()));
    }
}

/// A no-op [`IdempotencyStore`] — every `begin` is [`KeyState::Fresh`] and nothing is
/// retained. This is the "idempotency off" store: dispatch paths that don't opt in (and
/// the tests that don't exercise dedupe) pass it so there is one dispatch code path, not a
/// with/without-store fork. A [`crate::plan::Request`] with no key also short-circuits the
/// store entirely, so `NoStore` is only ever consulted for a keyless request in practice.
#[derive(Default)]
pub struct NoStore;

impl IdempotencyStore for NoStore {
    fn begin(&self, _callable: &str, _key: &str, _fingerprint: Fingerprint) -> KeyState {
        KeyState::Fresh
    }
    fn record(&self, _callable: &str, _key: &str, _response: serde_json::Value) {}
    fn abandon(&self, _callable: &str, _key: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A stand-in fingerprint for tests that don't exercise the mismatch path (the exact
    // value is opaque — only equality matters).
    const FP: Fingerprint = 1;

    #[test]
    fn fresh_then_done_replays() {
        let s = MemStore::new();
        assert_eq!(s.begin("m", "k1", FP), KeyState::Fresh);
        // While in flight a concurrent begin (same fingerprint) is blocked.
        assert_eq!(s.begin("m", "k1", FP), KeyState::InFlight);
        s.record("m", "k1", json!({ "id": "a" }));
        // Once recorded, replay the stored response for the same fingerprint.
        assert_eq!(s.begin("m", "k1", FP), KeyState::Done(json!({ "id": "a" })));
    }

    #[test]
    fn abandon_frees_the_key_for_retry() {
        let s = MemStore::new();
        assert_eq!(s.begin("m", "k", FP), KeyState::Fresh);
        s.abandon("m", "k");
        // Abandoned → a retry sees it fresh again.
        assert_eq!(s.begin("m", "k", FP), KeyState::Fresh);
    }

    #[test]
    fn key_is_scoped_to_the_callable() {
        let s = MemStore::new();
        s.begin("m1", "shared", FP);
        s.record("m1", "shared", json!(1));
        // The same key on a *different* mutation is independent.
        assert_eq!(s.begin("m2", "shared", FP), KeyState::Fresh);
    }

    #[test]
    fn different_fingerprint_on_a_done_key_is_a_mismatch() {
        let s = MemStore::new();
        s.begin("m", "k", FP);
        s.record("m", "k", json!({ "id": "a" }));
        // Same key, *different* request payload → reject rather than replay the wrong result.
        assert_eq!(s.begin("m", "k", FP + 1), KeyState::Mismatch);
        // The original fingerprint still replays — the mismatch didn't corrupt the entry.
        assert_eq!(s.begin("m", "k", FP), KeyState::Done(json!({ "id": "a" })));
    }

    #[test]
    fn different_fingerprint_on_an_in_flight_key_is_a_mismatch() {
        let s = MemStore::new();
        s.begin("m", "k", FP);
        // A concurrent claim under the same key but a different payload is a mismatch, not
        // an in-flight block (a genuine retry would carry the same fingerprint).
        assert_eq!(s.begin("m", "k", FP + 1), KeyState::Mismatch);
    }

    #[test]
    fn an_expired_key_reads_as_fresh() {
        let s = MemStore::with_ttl(Duration::from_millis(30));
        assert_eq!(s.begin("m", "k", FP), KeyState::Fresh);
        s.record("m", "k", json!({ "id": "a" }));
        // Within the TTL the recorded response still replays.
        assert_eq!(s.begin("m", "k", FP), KeyState::Done(json!({ "id": "a" })));
        std::thread::sleep(Duration::from_millis(60));
        // Past the TTL the key is gone → a retry runs fresh, not a stale replay.
        assert_eq!(s.begin("m", "k", FP), KeyState::Fresh);
    }

    #[test]
    fn the_sweep_reclaims_keys_that_are_never_revisited() {
        let s = MemStore::with_ttl(Duration::from_millis(30));
        s.begin("m", "k1", FP);
        s.begin("m", "k2", FP);
        assert_eq!(s.entry_count(), 2);
        std::thread::sleep(Duration::from_millis(60));
        // A later begin (past the sweep interval) evicts the two expired keys it never
        // touches, so memory doesn't accumulate on a long-lived store.
        s.begin("m", "k3", FP);
        assert_eq!(s.entry_count(), 1);
    }

    #[test]
    fn no_store_is_always_fresh() {
        let s = NoStore;
        assert_eq!(s.begin("m", "k", FP), KeyState::Fresh);
        s.record("m", "k", json!(1));
        // Nothing retained: still fresh (even under a different fingerprint).
        assert_eq!(s.begin("m", "k", FP + 1), KeyState::Fresh);
    }
}
