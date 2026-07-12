//! The engine that turns a wire request into a bound, executable statement and shapes
//! the result.
//!
//! The runtime is **in-process**: it holds the same [`CheckedSchema`] the compiler
//! produced and reuses codegen's one lowering ([`based_codegen::sql::lower_queries`])
//! rather than re-deriving SQL, so the executed SQL and its bind surface never drift
//! from what `based gen sql` emits.
//!
//! ## Request → response (read path)
//! 1. [`load::Compiled::load`] runs the front end (discover → parse → check) and
//!    lowers every query once.
//! 2. [`plan::plan_query`] validates the request's args against the signature, threads
//!    `$ctx`, binds the `:name` placeholders to positional values, and picks the
//!    response [`plan::Envelope`] from the query's inferred cardinality / pagination.
//! 3. [`run::run_query`] executes via an abstract [`run::Db`] and shapes the rows into
//!    the JSON envelope (`Option` for `get`, an array for `list`, the `{ rows, cursor }`
//!    page envelope for a paginated `list`).
//!
//! ## Write path (mutations)
//! [`plan::plan_mutation`] mirrors the read path, then generates each `create`'s engine
//! `id` ([`id::IdGen`]) and binds every write statement positionally (a `^.id`
//! back-reference reuses the value its create generated). [`run::run_mutation`] executes
//! the writes in order under one engine-owned transaction.
//!
//! A mutation may carry an **idempotency key** ([`idempotency`]): a keyed mutation runs
//! its write body at most once per key — a retry replays the first attempt's stored
//! response. The key is out-of-band request metadata (the `Idempotency-Key` header),
//! never the body or a schema field.
//!
//! ## Wire + driver
//! [`serve::dispatch`] is the wire surface: it routes `POST /q|m/<name>` → the callable,
//! runs it, and maps every outcome to a [`serve::WireResponse`] — a pure core testable
//! against [`run::MockDb`], no socket. A boundary [`plan::PlanError`] maps to `4xx`, a
//! [`run::DbError`] to a retryable `503`.
//!
//! Execution is native async over the [`run::DbRead`]/[`run::Db`]/[`run::Tx`]/
//! [`run::Backend`] traits: `fetch` always returns a row stream (a one-shot response is
//! a collect at the dispatch layer), and a transaction is a consuming typestate —
//! dropped without commit it rolls back, so an open tx never re-enters the pool. The
//! concrete drivers run over sqlx as a pure executor/pool layer: [`driver::ShardRouter`]
//! (MariaDB) and [`postgres::PgRouter`] are the scale-out seams — one bounded pool per
//! physical shard, single-shard dispatch by a stable logical-shard hash.
//!
//! ## The socket edge (feature `serve`)
//! [`http::serve`] is the HTTP listener (`based serve`): an axum service. `$ctx` comes
//! from headers via a pluggable [`http::ContextSource`], never the body. The edge
//! depends only on the driver-neutral [`run::Backend`] seam, so a second backend drops
//! in without a change here. [`sqlite::SqliteBackend`] is an infra-free in-memory
//! `Db`/`Backend` for end-to-end integration tests against a genuine engine.
//!
//! ## The in-process door
//! [`embed::Engine`] is the library twin of the HTTP edge: a [`Compiled`] schema over a
//! [`run::Backend`] and an [`id::IdGen`], run straight through the same
//! [`serve::dispatch`] core with no socket. `Send + Sync`, checkout-per-call — safe to
//! `Arc` into shared app state. It backs the same typed generated client
//! (`based gen client`).

pub mod cursor;
pub mod embed;
pub mod id;
pub mod idempotency;
pub mod load;
pub mod migrate;
pub mod plan;
pub mod run;
pub mod scan;
pub mod serve;
pub mod shard;
pub mod value;

#[cfg(feature = "mariadb")]
pub mod driver;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "serve")]
pub mod http;

pub use embed::Engine;
pub use id::{IdGen, SeqIdGen};
pub use idempotency::{Fingerprint, IdempotencyStore, KeyState, MemStore, NoStore};
pub use load::Compiled;
pub use migrate::{
    applied as applied_migrations, apply as apply_migrations, ensure_ledger, load_migrations,
    status as migration_status, ApplyOpts, ApplyReport, Direction, LedgerRow, MigrateError,
    MigrationState, PlannedMigration,
};
pub use plan::{
    plan_mutation, plan_query, Envelope, MutationPlan, PlanError, QueryPlan, Request, Stmt,
};
pub use run::{
    fetch_all, run_mutation, run_query, Backend, Db, DbError, DbErrorKind, DbRead, MockDb, Row,
    RowStream, RunError, Tx,
};
pub use serve::{dispatch, preflight, resolve_shard_key, WireResponse};
pub use value::SqlValue;

#[cfg(feature = "serve")]
pub use http::{
    serve, serve_with_handle, Context, ContextSource, Handle, HeaderView, ServeConfig, ServeError,
    TrustedHeaderContext,
};

#[cfg(feature = "sqlite")]
pub use sqlite::{SqliteBackend, SqliteDb};

#[cfg(feature = "postgres")]
pub use postgres::{PgRouter, PostgresDb};
