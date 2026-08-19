//! The host-language read-decide-write transaction seam (transactions.md, slice 1).
//!
//! The DSL stays non-Turing-complete: read-decide-write logic lives in host Rust, run
//! against an engine-owned transaction. A transaction is the *same generated client* over
//! a transaction-bound transport — every existing callable works inside a transaction with
//! no per-callable codegen, because the client is already generic over a `Transport`.
//!
//! This module is the runtime half: [`TxOptions`] (isolation + access mode), the
//! [`Transaction`] handle ([`Engine::begin`](crate::Engine::begin) opens one), and the
//! [`TxTransport`] the generated `Transport` impl runs callables through. The generated
//! half (the `Transport` impl, `client::transaction` / `transaction_retrying`, and the
//! `Transaction::client()` accessor) is emitted with the embedded bridge.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::embed::Engine;
use crate::run::{DbError, Tx};
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
            Some(tx) => {
                self.engine
                    .dispatch_on_tx(&mut **tx, route, args, ctx)
                    .await
            }
            None => WireResponse::error(
                500,
                "internal",
                "the transaction is already committed or rolled back".to_string(),
            ),
        }
    }
}
