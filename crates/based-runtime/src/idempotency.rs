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
//! dedupes, behind the same trait.

use std::collections::HashMap;
use std::sync::Mutex;

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

/// The state one key sits in inside a [`MemStore`]. Each variant carries the
/// [`Fingerprint`] of the attempt that created it, so a later `begin` under a *different*
/// fingerprint is caught as a [`KeyState::Mismatch`] instead of being replayed/blocked.
enum Entry {
    /// A `begin` has claimed it; no response recorded yet. Holds the claiming attempt's
    /// fingerprint.
    InFlight(Fingerprint),
    /// A response has been recorded; `begin` replays it for a matching fingerprint. Holds
    /// the completed attempt's fingerprint alongside its response.
    Done(Fingerprint, serde_json::Value),
}

/// An in-process [`IdempotencyStore`]: a `Mutex`-guarded map keyed by `(callable, key)`.
///
/// Correct for a single app instance (one process dedupes its own retries). It is
/// `Send + Sync`, so the shared HTTP worker pool uses one behind an `Arc`. A
/// multi-instance deployment wants a shared store (so a retry on another instance also
/// dedupes) behind the same trait. Keys accumulate (no eviction); a production store adds
/// a TTL. For local/embedded use and tests this is complete.
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
    fn begin(&self, callable: &str, key: &str, fingerprint: Fingerprint) -> KeyState {
        let mut map = self.entries.lock().expect("idempotency store poisoned");
        let k = (callable.to_string(), key.to_string());
        match map.get(&k) {
            None => {
                map.insert(k, Entry::InFlight(fingerprint));
                KeyState::Fresh
            }
            // A key seen before only replays/blocks for the *same* request payload; a
            // different fingerprint is one key reused for two different requests → reject.
            Some(Entry::InFlight(fp)) if *fp == fingerprint => KeyState::InFlight,
            Some(Entry::Done(fp, resp)) if *fp == fingerprint => KeyState::Done(resp.clone()),
            Some(_) => KeyState::Mismatch,
        }
    }

    fn record(&self, callable: &str, key: &str, response: serde_json::Value) {
        let mut map = self.entries.lock().expect("idempotency store poisoned");
        let k = (callable.to_string(), key.to_string());
        // Preserve the fingerprint the `begin` claim recorded (a `record` always follows a
        // `Fresh` claim for the same request). If the claim is somehow gone, fall back to a
        // fingerprint that never matches a future `begin`, so a stray record can't be
        // replayed under a mismatched payload.
        let fp = match map.get(&k) {
            Some(Entry::InFlight(fp)) | Some(Entry::Done(fp, _)) => *fp,
            None => Fingerprint::MAX,
        };
        map.insert(k, Entry::Done(fp, response));
    }

    fn abandon(&self, callable: &str, key: &str) {
        let mut map = self.entries.lock().expect("idempotency store poisoned");
        map.remove(&(callable.to_string(), key.to_string()));
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
    fn no_store_is_always_fresh() {
        let s = NoStore;
        assert_eq!(s.begin("m", "k", FP), KeyState::Fresh);
        s.record("m", "k", json!(1));
        // Nothing retained: still fresh (even under a different fingerprint).
        assert_eq!(s.begin("m", "k", FP + 1), KeyState::Fresh);
    }
}
