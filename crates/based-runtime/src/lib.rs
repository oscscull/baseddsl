//! The engine that turns a wire request into a bound, executable statement and shapes
//! the result.
//!
//! The runtime is **in-process**: it holds the [`CheckedSchema`] the compiler produced
//! and reuses codegen's lowering ([`based_codegen::sql::lower_queries`]), so executed SQL
//! never drifts from what `based gen sql` emits.
//!
//! ## Request → response (read path)
//! 1. [`load::Compiled::load`] runs the front end (discover → parse → check) and lowers
//!    every query once.
//! 2. [`plan::plan_query`] validates the request's args against the signature, threads
//!    `$ctx`, binds the `:name` placeholders to positional values, and picks the response
//!    [`plan::Envelope`] from the query's cardinality / pagination.
//! 3. [`run::run_query`] executes via an abstract [`run::Db`] and shapes the rows into the
//!    JSON envelope (`Option` for `get`, an array for `list`, `{ rows, cursor }` for a
//!    paginated `list`).
//!
//! ## Write path (mutations)
//! [`plan::plan_mutation`] mirrors the read path, then generates each `create`'s engine
//! `id` ([`id::IdGen`]) and binds every write positionally (a `$name.id` step reference
//! reuses the value its create generated). [`run::run_mutation`] executes the writes in
//! order under one engine-owned transaction.
//!
//! A mutation may carry an **idempotency key** ([`idempotency`]): the write body runs at
//! most once per key, and a retry replays the first attempt's stored response. The key is
//! out-of-band request metadata (the `Idempotency-Key` header).
//!
//! A mutation may declare a **`guard`** ([`guard`]): the app registers the named host
//! async fn ([`guard::Guards`] → [`embed::Engine::with_guards`]) and dispatch invokes it
//! before the write body on every door. A denial is a `403`; a declared-but-unregistered
//! guard refuses to build.
//!
//! ## Wire + driver
//! [`serve::dispatch`] is the wire surface: it routes `POST /q|m/<name>` → the callable,
//! runs it, and maps every outcome to a [`serve::WireResponse`] — a pure core testable
//! against [`run::MockDb`]. A [`plan::PlanError`] maps to `4xx`, a [`run::DbError`] to a
//! retryable `503`.
//!
//! Execution is native async over the [`run::DbRead`]/[`run::Db`]/[`run::Tx`]/
//! [`run::Backend`] traits: `fetch` returns a row stream, and a transaction is a consuming
//! typestate — dropped without commit it rolls back. The concrete drivers run over sqlx:
//! [`driver::ShardRouter`] (MariaDB) and [`postgres::PgRouter`] are the scale-out seams —
//! one bounded pool per physical shard, single-shard dispatch by a stable logical-shard
//! hash.
//!
//! ## The socket edge (feature `serve`)
//! [`http::serve`] is the HTTP listener (`based serve`): an axum service. `$ctx` comes from
//! headers via a pluggable [`http::ContextSource`]. The edge depends only on the
//! [`run::Backend`] seam, so a second backend drops in without a change here.
//! [`sqlite::SqliteBackend`] is an infra-free in-memory `Backend` for end-to-end tests.
//!
//! ## The in-process door
//! [`embed::Engine`] is the library twin of the HTTP edge: a [`Compiled`] schema over a
//! [`run::Backend`] and an [`id::IdGen`], run through the same [`serve::dispatch`] core
//! with no socket. `Send + Sync`, checkout-per-call. It backs the same typed generated
//! client (`based gen client`).

pub mod cursor;
pub mod embed;
pub mod guard;
pub mod id;
pub mod idempotency;
pub mod load;
pub mod migrate;
pub mod plan;
pub mod run;
pub mod scan;
pub mod serve;
pub mod shard;
pub mod tx;
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
pub use guard::{GuardRequest, GuardSetupError, GuardVerdict, Guards};
pub use id::{IdGen, SeqIdGen};
pub use idempotency::{
    DbStore, Fingerprint, IdempotencyStore, KeyState, MemStore, NoStore, TxClaim, TxIdempotency,
};
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
    fetch_all, run_mutation, run_mutation_on, run_query, run_query_stream, Backend, Db, DbError,
    DbErrorKind, DbRead, MockDb, Row, RowStream, RunError, ShapedStream, Tx,
};
pub use serve::{
    dispatch, dispatch_on, dispatch_stream, preflight, resolve_shard_key, WireResponse,
};
pub use tx::{AccessMode, AdoptedTransport, Isolation, Transaction, TxOptions, TxTransport};
pub use value::SqlValue;

// Re-export sqlx (present whenever a concrete driver is on) so the generated per-driver
// `adopt_*` constructors can name `based_runtime::sqlx::Transaction<'_, …>` without the
// consumer pinning a matching sqlx version themselves — the adopted transaction and the
// engine then speak the identical `sqlx` types.
#[cfg(any(feature = "mariadb", feature = "postgres", feature = "sqlite"))]
pub use sqlx;

#[cfg(feature = "serve")]
pub use http::{
    serve, serve_with_handle, serve_with_store, Context, ContextSource, Handle, HeaderView,
    ServeConfig, ServeError, TrustedHeaderContext,
};

#[cfg(feature = "sqlite")]
pub use sqlite::{AdoptedSqlite, SqliteBackend, SqliteDb};

#[cfg(feature = "postgres")]
pub use postgres::{AdoptedPg, PgRouter, PostgresDb};

#[cfg(feature = "mariadb")]
pub use driver::AdoptedMaria;
