//! The host-language read-decide-write transaction seam (transactions.md).
//!
//! The DSL stays non-Turing-complete: read-decide-write logic lives in host Rust, run
//! against a transaction. A transaction is the *same generated client* over a
//! transaction-bound transport — every existing callable works inside a transaction with
//! no per-callable codegen, because the client is already generic over a `Transport`.
//!
//! This module is the runtime half of all three rungs: [`TxOptions`] (isolation + access
//! mode), the engine-owned [`Transaction`] handle ([`Engine::begin`](crate::Engine::begin)
//! opens one) and the [`TxTransport`] it runs callables through (rungs 1–2), and the
//! [`AdoptedTransport`] that runs callables on a **caller-owned** transaction adopted from
//! the caller's own driver (rung 3, BYO `adopt` — the caller commits, `adopt` never does).
//! The generated half (the `Transport` impls, `client::transaction` /
//! `transaction_retrying`, `Transaction::client()`, and the per-driver `adopt_*`
//! constructors) is emitted with the embedded bridge.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::embed::Engine;
use crate::run::{DbError, DbRead, Tx};
use crate::serve::WireResponse;

pub use based_codegen::{AccessMode, Isolation};

/// How a transaction runs: its isolation level and access mode. The default is
/// `ReadCommitted` + `ReadWrite` — the safe, ordinary transaction. Every transaction
/// entry point takes one; it is applied per dialect through the
/// [`Dialect`](based_codegen::Dialect) seam when the transaction opens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TxOptions {
    pub isolation: Isolation,
    pub access: AccessMode,
}

impl TxOptions {
    /// The default options (`ReadCommitted` + `ReadWrite`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the isolation level (builder style).
    #[must_use]
    pub fn isolation(mut self, isolation: Isolation) -> Self {
        self.isolation = isolation;
        self
    }

    /// Set the access mode (builder style).
    #[must_use]
    pub fn access(mut self, access: AccessMode) -> Self {
        self.access = access;
        self
    }

    /// `Serializable` isolation — the strongest level; the engine may abort with a
    /// serialization failure the caller retries (`transaction_retrying`).
    #[must_use]
    pub fn serializable(self) -> Self {
        self.isolation(Isolation::Serializable)
    }

    /// `ReadOnly` access — a write inside the transaction is a database error.
    #[must_use]
    pub fn read_only(self) -> Self {
        self.access(AccessMode::ReadOnly)
    }
}

/// A shared slot holding the open transaction. The [`Transaction`] handle takes it out to
/// commit/rollback; the [`TxTransport`](TxTransport)(s) borrowing it run callables on it.
/// Wrapped in a [`tokio::sync::Mutex`] so it stays `Send` (for axum) while giving the
/// `&mut` a statement needs — calls are sequential/awaited, so there is no real contention.
type TxSlot = Arc<Mutex<Option<Box<dyn Tx>>>>;

/// An open, caller-owned transaction — the explicit-handle rung of the seam. Get a client
/// bound to it (`Transaction::client()`, emitted with the embedded bridge, or
/// [`Transaction::transport`]), run any callables, then [`commit`](Transaction::commit) or
/// [`rollback`](Transaction::rollback). **Dropping it without committing rolls back** (the
/// [`Tx`] typestate guarantees it), so a lost handle — or a panic — never leaks a
/// half-written transaction.
pub struct Transaction {
    engine: Engine,
    slot: TxSlot,
}

impl Transaction {
    pub(crate) fn new(engine: Engine, tx: Box<dyn Tx>) -> Self {
        Self {
            engine,
            slot: Arc::new(Mutex::new(Some(tx))),
        }
    }

    /// Commit the transaction, consuming the handle. A no-op if it was already
    /// committed/rolled back.
    pub async fn commit(self) -> Result<(), DbError> {
        match self.slot.lock().await.take() {
            Some(tx) => tx.commit().await,
            None => Ok(()),
        }
    }

    /// Roll the transaction back, consuming the handle. A no-op if it was already
    /// committed/rolled back. (Dropping the handle also rolls back — this is the explicit,
    /// awaited form.)
    pub async fn rollback(self) -> Result<(), DbError> {
        match self.slot.lock().await.take() {
            Some(tx) => tx.rollback().await,
            None => Ok(()),
        }
    }

    /// A [`TxTransport`] bound to this transaction — the transport a generated
    /// `Client` runs on. The generated `Transaction::client()` (embedded bridge) wraps
    /// this in a `Client`; callers using the runtime directly build `Client { transport }`.
    pub fn transport(&self) -> TxTransport {
        TxTransport {
            engine: self.engine.clone(),
            slot: self.slot.clone(),
        }
    }
}

/// A `Transport` bound to an open [`Transaction`]: every callable it runs executes on the
/// held transaction (through the engine's dispatch core), committing nothing — the
/// [`Transaction`] owns the boundary. Cheap to clone (an [`Engine`] handle + an `Arc`).
/// The generated module implements the `Transport` trait for this type (the trait is
/// defined in the generated client, so the impl must live there); [`TxTransport::dispatch`]
/// is the one runtime entry point that impl calls.
pub struct TxTransport {
    engine: Engine,
    slot: TxSlot,
}

impl TxTransport {
    /// Run one callable on the held transaction and return the wire response — the
    /// transaction-bound twin of [`Engine::call`](crate::Engine::call). The generated
    /// `Transport` impl serializes the typed input/`$ctx` to JSON, calls this, and decodes
    /// the `200` body (a non-`200` becomes the client's error), exactly like the `Embedded`
    /// bridge — only the connection differs (the held transaction, not a fresh checkout).
    pub async fn dispatch(&self, route: &str, args: Value, ctx: Value) -> WireResponse {
        let mut guard = self.slot.lock().await;
        match guard.as_mut() {
            Some(tx) => self.engine.dispatch_on(&mut **tx, route, args, ctx).await,
            None => WireResponse::error(
                500,
                "internal",
                "the transaction is already committed or rolled back".to_string(),
            ),
        }
    }
}

/// A `Transport` bound to a **caller-owned** open transaction the caller adopted from its
/// own driver — the bring-your-own (`adopt`) rung of the seam (transactions.md rung 3). It
/// holds a per-driver adapter (`D: DbRead`) over the caller's *borrowed* native
/// `sqlx::Transaction`, and every callable runs on it through the engine's dispatch core.
///
/// The defining property: **`adopt` never begins, commits, or rolls back** — the caller
/// owns the boundary, so dropping the adopted client leaves the caller's transaction
/// untouched (unlike [`TxTransport`], whose [`Transaction`] drop-rolls-back). The adopter
/// runs its raw writes and its baseddsl work on one transaction and commits it itself, so
/// both land atomically. `for update` locking reads work through it (the generated
/// `AdoptedTransport` carries the client's `TxBound` marker). There is no auto-retry
/// (`transaction_retrying`) here: retrying a whole transaction requires owning its boundary,
/// which the caller does.
///
/// Not cloned: the adopted [`crate::Engine`]-generated `Client` owns one transport by value,
/// holding the borrow for the transport's lifetime. The adapter sits behind a
/// [`tokio::sync::Mutex`] because `Transport::call` takes `&self` while running a statement
/// needs `&mut` — calls are sequential/awaited, so there is no real contention, and the
/// `Mutex` keeps the transport `Send` for axum.
pub struct AdoptedTransport<D> {
    engine: Engine,
    db: Mutex<D>,
}

impl<D: DbRead> AdoptedTransport<D> {
    /// Wrap an engine handle and a driver adapter over the caller's borrowed transaction.
    /// The per-driver `adopt_*` constructor the generated client emits builds the adapter
    /// (e.g. `AdoptedPg::new(&mut pg_tx)`) and hands it here.
    pub fn new(engine: Engine, db: D) -> Self {
        Self {
            engine,
            db: Mutex::new(db),
        }
    }

    /// Run one callable on the adopted transaction and return the wire response — the
    /// adopted twin of [`TxTransport::dispatch`]. Runs through the same
    /// [`Engine::dispatch_on`] core, which commits nothing (the caller owns the boundary).
    pub async fn dispatch(&self, route: &str, args: Value, ctx: Value) -> WireResponse {
        let mut guard = self.db.lock().await;
        self.engine.dispatch_on(&mut *guard, route, args, ctx).await
    }
}
