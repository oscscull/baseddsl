//! Executing a planned query and shaping the rows into the response envelope.
//!
//! Execution goes through the abstract [`DbRead`]/[`Db`]/[`Tx`]/[`Backend`] traits —
//! the runtime's twin of the generated client's abstract `Transport`; concrete drivers
//! (`sqlite`, `driver`, `postgres`) implement them, and a [`MockDb`] returns canned
//! rows so the whole request → JSON path is testable with no database. Row shaping is
//! where the envelope becomes real: `get` → a JSON object or `null`, `list` → an
//! array, a paginated `list` → the `{ rows, cursor }` page envelope (the keyset cursor
//! is minted here from the last row's hidden sort-key columns).
//!
//! Reads have exactly one path: [`DbRead::fetch`] returns a fallible row *stream*,
//! always — a one-shot response is a collect at this layer, and a streaming wire
//! surface consumes the same stream. Transactions are a consuming typestate:
//! [`Db::begin`] takes the connection, [`Tx::commit`] takes the transaction, and a
//! `Tx` dropped without commit rolls back or discards its connection — an open
//! transaction can never re-enter the pool, and a cancelled caller can never leave a
//! half-written mutation behind.

use async_trait::async_trait;

use crate::id::IdGen;
use crate::idempotency::{Fingerprint, IdempotencyStore, KeyState, TxClaim, TxIdempotency};
use crate::load::Compiled;
use crate::plan::{
    plan_mutation, plan_query, Envelope, KeysetPlan, MutationPlan, PlanError, QueryPlan, Request,
    Stmt,
};
use crate::value::SqlValue;
use based_codegen::sql::{ARRAY_MARK, KEYSET_PREFIX};

mod bulk;
mod db;
mod error;
mod mock;
mod mutation;
mod query;
mod shaping;

pub(crate) use bulk::*;
pub use db::*;
pub use error::*;
pub use mock::*;
pub use mutation::*;
pub use query::*;
pub(crate) use shaping::*;
