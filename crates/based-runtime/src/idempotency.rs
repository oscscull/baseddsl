//! Write-retry idempotency — dedupe a retried `create`/mutation.
//!
//! The engine mints a fresh `id` for every `create`, so a client that retries a mutation
//! after a `503`/timeout would double-insert. An idempotency key closes it: the caller
//! attaches a stable key to a mutation, and the engine runs the write body at most once
//! per key — a retry replays the first attempt's stored response.
//!
//! ## Scope
//! - Mutations only. A query is naturally idempotent, so only [`crate::run::run_mutation`]
//!   touches the store.
//! - Opt-in. No key → run every time. The key is request metadata, supplied out of band by
//!   the wire edge (`Idempotency-Key` header); a schema never reads it.
//! - Keyed by `(callable, key)`, so the same key reused across two mutations does not
//!   collide.
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
//!   reject with a retryable `409`.
//! - Mismatch → the key was seen before with a different fingerprint (the caller reused one
//!   key for two requests); reject with a non-retryable `422` rather than replay the wrong
//!   result.
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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::run::Backend;

/// How long a [`MemStore`] key is retained before it expires. A retried mutation arrives
/// within seconds of the first attempt, so a day is ample for dedupe while bounding how
/// long a key occupies memory (the industry-standard idempotency-key window).
const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// A stable hash of a request's args + `$ctx` — the payload a keyed mutation carries.
///
/// Genuine retries of the same request produce the same fingerprint; a key reused for two
/// different requests produces different ones, which the store rejects. Built by
/// [`Request::fingerprint`](crate::Request::fingerprint); opaque, compared only for
/// equality.
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
///
/// [`begin`](Self::begin)/[`record`](Self::record) are `async` — a networked store (Redis,
/// …) does its round-trip on the request's own task with no blocking bridge.
/// [`abandon`](Self::abandon) is the lone sync method **by necessity**: it is the release
/// hook the mutation's cancellation `Drop` guard fires, and `Drop` cannot `.await`. A store
/// that needs I/O to release implements it as fire-and-forget through its own owned handle
/// (see [`MemStore`] — an immediate in-memory removal — for the trivial case).
#[async_trait::async_trait]
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
    async fn begin(&self, callable: &str, key: &str, fingerprint: Fingerprint) -> KeyState;

    /// Record the successful response for a claimed key: future `begin`s replay it
    /// ([`KeyState::Done`]).
    async fn record(&self, callable: &str, key: &str, response: serde_json::Value);

    /// Release a claimed key without recording a response (the attempt failed or was
    /// cancelled): a later retry may re-run the write. Fired from the mutation's `Drop`
    /// guard, so it is sync and must not block — a store that needs I/O to release
    /// dispatches it fire-and-forget through its own owned handle.
    fn abandon(&self, callable: &str, key: &str);

    /// A store that commits the key **inside the mutation's own transaction** (atomic
    /// exactly-once across app instances) returns a [`TxIdempotency`] here; the default
    /// `None` is the out-of-band store (in-process [`MemStore`], a Redis store) whose
    /// [`begin`](Self::begin)/[`record`](Self::record)/[`abandon`](Self::abandon) bracket
    /// the mutation instead. When this is `Some`, [`crate::run::run_mutation`] ignores the
    /// out-of-band trio and drives claim/record on the mutation connection instead — so the
    /// two paths never mix for one store.
    fn tx_participant(&self) -> Option<&dyn TxIdempotency> {
        None
    }
}

/// The outcome of claiming an idempotency key on the mutation's own connection — the
/// tx-participant analogue of [`KeyState`], minus `InFlight`: a concurrent attempt does not
/// report a conflict, it *blocks* on the unclaimed row and then reads the committed result
/// (see [`TxIdempotency`]).
pub enum TxClaim {
    /// No committed prior attempt: this attempt holds the claim (its in-flight row is
    /// written in the mutation's transaction) and should run the write body.
    Fresh,
    /// A prior attempt with the **same fingerprint** already committed: replay this stored
    /// response, run nothing.
    Done(serde_json::Value),
    /// The key was seen before with a **different** fingerprint (one key reused for two
    /// requests): reject rather than replay the wrong result.
    Mismatch,
}

/// A durable store that commits the idempotency key **in the mutation's own transaction**,
/// giving genuine exactly-once even when a retry lands on a *different* app instance (the
/// key lives in a shared table, not per-process memory). Returned by
/// [`IdempotencyStore::tx_participant`].
///
/// Concurrency is **block-and-replay**, not 409: a concurrent retry's [`claim`](Self::claim)
/// blocks on the first attempt's still-uncommitted key row (the table's unique index) and,
/// once that transaction commits, reads and replays its recorded response — so this store
/// never emits a 409 conflict, at the cost of holding a database connection while blocked
/// (pool pressure under a retry storm). [`claim`](Self::claim) runs right after the
/// mutation's `begin`, [`record`](Self::record) right before its `commit`, both on the
/// mutation connection.
#[async_trait::async_trait]
pub trait TxIdempotency: Send + Sync {
    /// Claim `(callable, key)` on the mutation's connection, inside its transaction, before
    /// any write. A concurrent retry blocks here on the first, still-uncommitted attempt and
    /// resolves to [`TxClaim::Done`] once it commits.
    async fn claim(
        &self,
        db: &mut dyn crate::run::DbRead,
        callable: &str,
        key: &str,
        fingerprint: Fingerprint,
    ) -> Result<TxClaim, crate::run::DbError>;

    /// Record the response for a claimed key on the same connection, before the transaction
    /// commits — so the key, the writes, and the response commit atomically (or roll back
    /// together).
    async fn record(
        &self,
        db: &mut dyn crate::run::DbRead,
        callable: &str,
        key: &str,
        response: &serde_json::Value,
    ) -> Result<(), crate::run::DbError>;
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

#[async_trait::async_trait]
impl IdempotencyStore for MemStore {
    async fn begin(&self, callable: &str, key: &str, fingerprint: Fingerprint) -> KeyState {
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

    async fn record(&self, callable: &str, key: &str, response: serde_json::Value) {
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

#[async_trait::async_trait]
impl IdempotencyStore for NoStore {
    async fn begin(&self, _callable: &str, _key: &str, _fingerprint: Fingerprint) -> KeyState {
        KeyState::Fresh
    }
    async fn record(&self, _callable: &str, _key: &str, _response: serde_json::Value) {}
    fn abandon(&self, _callable: &str, _key: &str) {}
}

/// A durable [`IdempotencyStore`] that keeps its keys in a `_based_idempotency` table in
/// the same database the mutations write to, committing each key **in the mutation's own
/// transaction** — so a keyed retry that lands on a *different* app instance still
/// deduplicates, and the key can never commit apart from the writes it guards (genuine
/// exactly-once, the DB-first alternative to a Redis store). It is a [`TxIdempotency`], so
/// [`crate::run::run_mutation`] drives it inside the transaction rather than bracketing it;
/// its out-of-band [`begin`](IdempotencyStore::begin)/[`record`](IdempotencyStore::record)/
/// [`abandon`](IdempotencyStore::abandon) are never consulted (they exist only to satisfy
/// the one injection trait).
///
/// Inject it like any other store — [`Engine::with_store`](crate::Engine::with_store) or
/// [`serve_with_store`](crate::http::serve_with_store) — after
/// [`create`](Self::create)ing the key table. Concurrency is block-and-replay (see
/// [`TxIdempotency`]): this store never returns a 409.
///
/// ## Growth
/// Every keyed mutation leaves a row. Left unbounded that table only grows, so the store
/// is intended as a *basic* durable option, not a high-volume one — reach for an
/// out-of-band store with native expiry (a Redis [`IdempotencyStore`]) at scale. To keep
/// the table bounded here, build it with [`with_gc`](Self::with_gc): a **dumb, amortized,
/// age-based sweep** deletes rows past a TTL, on the store's *own* connection (never a
/// mutation's transaction), at most once per TTL window. [`create`](Self::create) /
/// [`new`](Self::new) keep every key forever (the retain-forever escape).
pub struct DbStore {
    dialect: based_codegen::Dialect,
    gc: Option<Gc>,
}

/// The age-based reclaim a [`DbStore::with_gc`] store runs: the connection source to sweep
/// on (its own, so a sweep never touches a mutation's transaction), the max key age, and
/// the next instant a sweep is due (amortized to one pass per TTL window).
struct Gc {
    backend: Arc<dyn Backend>,
    ttl: Duration,
    next_sweep: Mutex<Instant>,
}

impl DbStore {
    /// Build the store and ensure its key table exists (idempotent DDL, per dialect through
    /// the codegen seam). Run once at startup with the engine's backend + dialect. Keys are
    /// kept forever — use [`with_gc`](Self::with_gc) to bound the table by age.
    pub async fn create(
        backend: &dyn crate::run::Backend,
        dialect: based_codegen::Dialect,
    ) -> Result<Self, crate::run::DbError> {
        let mut db = backend.checkout("").await?;
        db.execute(&based_codegen::sql::idempotency_table_ddl(dialect), &[])
            .await?;
        Ok(Self { dialect, gc: None })
    }

    /// Build the store assuming the key table already exists (e.g. created by a migration
    /// or a prior [`create`](Self::create)). Keys are kept forever (no sweep).
    pub fn new(dialect: based_codegen::Dialect) -> Self {
        Self { dialect, gc: None }
    }

    /// Build the store with an **age-based sweep** so the key table stays bounded: it
    /// ensures the table exists, then deletes rows older than `ttl` on its own connection,
    /// at most once per `ttl` window (a dumb amortized reclaim, no background task). Retain
    /// the `backend` for the sweeps; a keyed mutation runs the reclaim inline when one is
    /// due, so the cost is one bulk `DELETE` per window and never touches a mutation's
    /// transaction. A day is a typical `ttl` (the idempotency-key retry window). For
    /// retain-forever, use [`create`](Self::create) instead.
    ///
    /// Single-shard: the sweep runs on the empty shard key, so under a sharded backend it
    /// reclaims only that shard's table (each database keeps its own key rows).
    pub async fn with_gc(
        backend: impl Backend + 'static,
        dialect: based_codegen::Dialect,
        ttl: Duration,
    ) -> Result<Self, crate::run::DbError> {
        let backend: Arc<dyn Backend> = Arc::new(backend);
        let mut db = backend.checkout("").await?;
        db.execute(&based_codegen::sql::idempotency_table_ddl(dialect), &[])
            .await?;
        Ok(Self {
            dialect,
            gc: Some(Gc {
                backend,
                ttl,
                next_sweep: Mutex::new(Instant::now() + ttl),
            }),
        })
    }

    /// When a sweep is due, spawn a **detached** age-based reclaim on the store's own
    /// connection — so it never blocks, stalls, or fails the mutation that triggered it
    /// (GC is pure best-effort). Amortized to one pass per TTL window. A no-op unless the
    /// store was built with [`with_gc`](Self::with_gc), or if called outside a runtime.
    fn maybe_sweep(&self) {
        let Some(gc) = self.gc.as_ref() else {
            return;
        };
        let now = Instant::now();
        {
            let mut next = gc
                .next_sweep
                .lock()
                .expect("idempotency sweep clock poisoned");
            if now < *next {
                return;
            }
            *next = now + gc.ttl;
        }
        let backend = gc.backend.clone();
        let sql = self.delete_expired_sql(gc.ttl);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Ok(mut db) = backend.checkout("").await {
                    let _ = db.execute(&sql, &[]).await;
                }
            });
        }
    }

    /// `DELETE` of every key row older than `ttl`, measured against the database clock so a
    /// client/DB skew can't spare or over-reap rows. The interval is the store's own
    /// [`Duration`] (never caller input), so its seconds inline safely.
    fn delete_expired_sql(&self, ttl: Duration) -> String {
        use based_codegen::Dialect as D;
        let secs = ttl.as_secs();
        let tbl = self.dialect.quote(based_codegen::sql::IDEMPOTENCY_TABLE);
        let created = self.dialect.quote("created_at");
        match self.dialect {
            D::Postgres => format!(
                "DELETE FROM {tbl} WHERE {created} < CURRENT_TIMESTAMP - INTERVAL '{secs} seconds'"
            ),
            D::MariaDb | D::MySql => {
                format!("DELETE FROM {tbl} WHERE {created} < CURRENT_TIMESTAMP - INTERVAL {secs} SECOND")
            }
            D::Sqlite => format!(
                "DELETE FROM {tbl} WHERE {created} < datetime(CURRENT_TIMESTAMP, '-{secs} seconds')"
            ),
        }
    }

    /// `?` for MySQL/MariaDB/SQLite, `$n` for Postgres — the driver's positional bind form.
    fn ph(&self, n: usize) -> String {
        match self.dialect {
            based_codegen::Dialect::Postgres => format!("${n}"),
            _ => "?".to_string(),
        }
    }

    /// Insert the in-flight claim row, ignoring a duplicate key (the dialect's
    /// insert-or-ignore): 1 row affected ⇒ we claimed it fresh, 0 ⇒ a prior attempt holds
    /// the key. `created_at` is the DB clock; `response` defaults to NULL until `record`.
    fn insert_sql(&self) -> String {
        use based_codegen::Dialect as D;
        let q = |s: &str| self.dialect.quote(s);
        let cols = format!(
            "({}, {}, {}, {})",
            q("callable"),
            q("key"),
            q("fingerprint"),
            q("created_at"),
        );
        let vals = format!(
            "({}, {}, {}, CURRENT_TIMESTAMP)",
            self.ph(1),
            self.ph(2),
            self.ph(3),
        );
        let tbl = q(based_codegen::sql::IDEMPOTENCY_TABLE);
        match self.dialect {
            D::Postgres => format!(
                "INSERT INTO {tbl} {cols} VALUES {vals} ON CONFLICT ({}, {}) DO NOTHING",
                q("callable"),
                q("key"),
            ),
            D::Sqlite => format!("INSERT OR IGNORE INTO {tbl} {cols} VALUES {vals}"),
            D::MariaDb | D::MySql => format!("INSERT IGNORE INTO {tbl} {cols} VALUES {vals}"),
        }
    }

    fn select_sql(&self) -> String {
        let q = |s: &str| self.dialect.quote(s);
        format!(
            "SELECT {fp}, {resp} FROM {tbl} WHERE {c} = {p1} AND {k} = {p2}",
            fp = q("fingerprint"),
            resp = q("response"),
            tbl = q(based_codegen::sql::IDEMPOTENCY_TABLE),
            c = q("callable"),
            k = q("key"),
            p1 = self.ph(1),
            p2 = self.ph(2),
        )
    }

    fn update_sql(&self) -> String {
        let q = |s: &str| self.dialect.quote(s);
        format!(
            "UPDATE {tbl} SET {resp} = {p1} WHERE {c} = {p2} AND {k} = {p3}",
            tbl = q(based_codegen::sql::IDEMPOTENCY_TABLE),
            resp = q("response"),
            c = q("callable"),
            k = q("key"),
            p1 = self.ph(1),
            p2 = self.ph(2),
            p3 = self.ph(3),
        )
    }
}

#[async_trait::async_trait]
impl IdempotencyStore for DbStore {
    // Unused: a tx-participant store deduplicates inside the mutation's transaction (see
    // `tx_participant`), so `run_mutation` never takes the out-of-band path for it. These
    // exist only to satisfy the one injection trait.
    async fn begin(&self, _callable: &str, _key: &str, _fingerprint: Fingerprint) -> KeyState {
        KeyState::Fresh
    }
    async fn record(&self, _callable: &str, _key: &str, _response: serde_json::Value) {}
    fn abandon(&self, _callable: &str, _key: &str) {}

    fn tx_participant(&self) -> Option<&dyn TxIdempotency> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl TxIdempotency for DbStore {
    async fn claim(
        &self,
        db: &mut dyn crate::run::DbRead,
        callable: &str,
        key: &str,
        fingerprint: Fingerprint,
    ) -> Result<TxClaim, crate::run::DbError> {
        use crate::run::DbError;
        use crate::value::SqlValue;
        // Amortized age-based reclaim (a no-op unless built with `with_gc`): fire-and-forget
        // on its own connection, so it never disturbs this mutation's transaction below.
        self.maybe_sweep();
        let fp = fingerprint.to_string();
        let affected = db
            .execute(
                &self.insert_sql(),
                &[
                    SqlValue::Text(callable.to_string()),
                    SqlValue::Text(key.to_string()),
                    SqlValue::Text(fp.clone()),
                ],
            )
            .await?;
        if affected >= 1 {
            // We inserted the in-flight row: the claim is ours.
            return Ok(TxClaim::Fresh);
        }
        // A prior attempt holds the key. Because a concurrent attempt's row is invisible
        // until it commits (and our insert blocks until then), a visible row here always
        // has its response recorded.
        let rows = crate::run::fetch_all(db.fetch(
            &self.select_sql(),
            &[
                SqlValue::Text(callable.to_string()),
                SqlValue::Text(key.to_string()),
            ],
        ))
        .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| DbError::new("idempotency key row vanished after a conflict"))?;
        let stored_fp = row
            .get("fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if stored_fp != fp {
            return Ok(TxClaim::Mismatch);
        }
        match row.get("response") {
            // Stored as text (a TEXT column): parse the recorded JSON response.
            Some(serde_json::Value::String(s)) => serde_json::from_str(s)
                .map(TxClaim::Done)
                .map_err(|e| DbError::new(format!("stored idempotency response is not JSON: {e}"))),
            // A driver that decoded the column natively hands back the value directly.
            Some(other) if !other.is_null() => Ok(TxClaim::Done(other.clone())),
            _ => Err(DbError::new(
                "idempotency claim conflicted with a committed row that recorded no response",
            )),
        }
    }

    async fn record(
        &self,
        db: &mut dyn crate::run::DbRead,
        callable: &str,
        key: &str,
        response: &serde_json::Value,
    ) -> Result<(), crate::run::DbError> {
        use crate::value::SqlValue;
        db.execute(
            &self.update_sql(),
            &[
                SqlValue::Text(response.to_string()),
                SqlValue::Text(callable.to_string()),
                SqlValue::Text(key.to_string()),
            ],
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A stand-in fingerprint for tests that don't exercise the mismatch path (the exact
    // value is opaque — only equality matters).
    const FP: Fingerprint = 1;

    #[tokio::test]
    async fn fresh_then_done_replays() {
        let s = MemStore::new();
        assert_eq!(s.begin("m", "k1", FP).await, KeyState::Fresh);
        // While in flight a concurrent begin (same fingerprint) is blocked.
        assert_eq!(s.begin("m", "k1", FP).await, KeyState::InFlight);
        s.record("m", "k1", json!({ "id": "a" })).await;
        // Once recorded, replay the stored response for the same fingerprint.
        assert_eq!(
            s.begin("m", "k1", FP).await,
            KeyState::Done(json!({ "id": "a" }))
        );
    }

    #[tokio::test]
    async fn abandon_frees_the_key_for_retry() {
        let s = MemStore::new();
        assert_eq!(s.begin("m", "k", FP).await, KeyState::Fresh);
        s.abandon("m", "k");
        // Abandoned → a retry sees it fresh again.
        assert_eq!(s.begin("m", "k", FP).await, KeyState::Fresh);
    }

    #[tokio::test]
    async fn key_is_scoped_to_the_callable() {
        let s = MemStore::new();
        s.begin("m1", "shared", FP).await;
        s.record("m1", "shared", json!(1)).await;
        // The same key on a *different* mutation is independent.
        assert_eq!(s.begin("m2", "shared", FP).await, KeyState::Fresh);
    }

    #[tokio::test]
    async fn different_fingerprint_on_a_done_key_is_a_mismatch() {
        let s = MemStore::new();
        s.begin("m", "k", FP).await;
        s.record("m", "k", json!({ "id": "a" })).await;
        // Same key, *different* request payload → reject rather than replay the wrong result.
        assert_eq!(s.begin("m", "k", FP + 1).await, KeyState::Mismatch);
        // The original fingerprint still replays — the mismatch didn't corrupt the entry.
        assert_eq!(
            s.begin("m", "k", FP).await,
            KeyState::Done(json!({ "id": "a" }))
        );
    }

    #[tokio::test]
    async fn different_fingerprint_on_an_in_flight_key_is_a_mismatch() {
        let s = MemStore::new();
        s.begin("m", "k", FP).await;
        // A concurrent claim under the same key but a different payload is a mismatch, not
        // an in-flight block (a genuine retry would carry the same fingerprint).
        assert_eq!(s.begin("m", "k", FP + 1).await, KeyState::Mismatch);
    }

    #[tokio::test]
    async fn an_expired_key_reads_as_fresh() {
        let s = MemStore::with_ttl(Duration::from_millis(30));
        assert_eq!(s.begin("m", "k", FP).await, KeyState::Fresh);
        s.record("m", "k", json!({ "id": "a" })).await;
        // Within the TTL the recorded response still replays.
        assert_eq!(
            s.begin("m", "k", FP).await,
            KeyState::Done(json!({ "id": "a" }))
        );
        std::thread::sleep(Duration::from_millis(60));
        // Past the TTL the key is gone → a retry runs fresh, not a stale replay.
        assert_eq!(s.begin("m", "k", FP).await, KeyState::Fresh);
    }

    #[tokio::test]
    async fn the_sweep_reclaims_keys_that_are_never_revisited() {
        let s = MemStore::with_ttl(Duration::from_millis(30));
        s.begin("m", "k1", FP).await;
        s.begin("m", "k2", FP).await;
        assert_eq!(s.entry_count(), 2);
        std::thread::sleep(Duration::from_millis(60));
        // A later begin (past the sweep interval) evicts the two expired keys it never
        // touches, so memory doesn't accumulate on a long-lived store.
        s.begin("m", "k3", FP).await;
        assert_eq!(s.entry_count(), 1);
    }

    #[tokio::test]
    async fn no_store_is_always_fresh() {
        let s = NoStore;
        assert_eq!(s.begin("m", "k", FP).await, KeyState::Fresh);
        s.record("m", "k", json!(1)).await;
        // Nothing retained: still fresh (even under a different fingerprint).
        assert_eq!(s.begin("m", "k", FP + 1).await, KeyState::Fresh);
    }
}
