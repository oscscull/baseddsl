//! based-runtime (M6) — the engine that turns a wire request into a bound,
//! executable statement and shapes the result.
//!
//! The runtime is **in-process**: it holds the same [`CheckedSchema`] the compiler
//! produced and reuses codegen's *one* lowering ([`based_codegen::sql::lower_queries`])
//! rather than re-deriving SQL or reading a serialized artifact. So the executed SQL
//! and its bind surface can never drift from what `based gen sql` emits (principle 4).
//!
//! ## Request → response (read path, this slice)
//! 1. [`load::Compiled::load`] runs the front end (discover → parse → check) and
//!    lowers every query once.
//! 2. [`plan::plan_query`] validates the request's args against the signature
//!    (required / defaults / coercion, calling.md #3), threads `$ctx` (the
//!    per-callable requirement bag, D4/D5), binds the `:name` placeholders to
//!    positional `?` values, and picks the response [`plan::Envelope`] from the
//!    query's inferred cardinality / pagination.
//! 3. [`run::run_query`] executes via an abstract [`run::Db`] and shapes the rows
//!    into the JSON envelope (`Option` for `get`, an array for `list`, the
//!    `{ rows, cursor }` page envelope for a paginated `list`).
//!
//! ## Write path (mutations)
//! [`plan::plan_mutation`] mirrors the read path: it validates args + `$ctx`, then
//! generates each `create`'s engine `id` ([`id::IdGen`], D1) and binds every write
//! statement positionally (a `^.id` back-reference reuses the value its create
//! generated). [`run::run_mutation`] executes the writes in order under one
//! engine-owned transaction (principle 7 — [`run::Db`] grows `execute`/`begin`/
//! `commit`) and returns the write response. Codegen's [`based_codegen::sql::lower_mutations`]
//! is the one lowering both `based gen sql` and the runtime read, so the executed
//! writes can never drift from the emitted SQL either.
//!
//! ## Wire + driver
//! [`serve::dispatch`] is the wire surface: it routes `POST /q|m/<name>` → the callable,
//! runs it, and maps every outcome to a [`serve::WireResponse`] (HTTP status + JSON) —
//! a pure core testable against [`run::MockDb`], no socket. Every [`run::Db`] method is
//! **fallible** (a dependable driver surfaces failures, not panics); a boundary
//! [`plan::PlanError`] maps to `4xx`, a [`run::DbError`] to a retryable `503`.
//!
//! The concrete [`driver::MariaDb`] (feature `mariadb`) is the production `Db` over one
//! pooled connection, and [`driver::ShardRouter`] is the scale-out seam: one bounded
//! pool per physical shard, single-shard dispatch by a stable logical-shard hash (no
//! scatter-gather → a `tx` is one shard, no distributed transaction; add capacity
//! without rehashing keys).
//!
//! ## The socket edge (feature `serve`, D21)
//! [`http::serve`] is the HTTP listener (`based serve`): a sync bounded worker-thread
//! pool over `tiny_http`, decoding each request into `dispatch`'s arguments. `$ctx`
//! comes from headers via a pluggable [`http::ContextSource`], never the body
//! (auth.md/D7); ids come from the production [`id::UuidGen`]. The edge depends only on
//! the driver-neutral [`run::Backend`] seam (a connection source yielding a boxed
//! [`run::Db`]), so a future Postgres / MySQL / SQLite backend drops in without a change
//! here — the `Db` trait speaks positional SQL + [`value::SqlValue`], not a MariaDB
//! protocol (multi-dialect readiness, D21).
//!
//! The write response is the created row's engine `id` today — the declared-shape
//! re-select (RETURNING) is deferred (D12).

pub mod id;
pub mod load;
pub mod plan;
pub mod run;
pub mod scan;
pub mod serve;
pub mod value;

#[cfg(feature = "mariadb")]
pub mod driver;

#[cfg(feature = "serve")]
pub mod http;

pub use id::{IdGen, SeqIdGen};
pub use load::Compiled;
pub use plan::{
    plan_mutation, plan_query, Envelope, MutationPlan, PlanError, QueryPlan, Request, Stmt,
};
pub use run::{run_mutation, run_query, Backend, Db, DbError, MockDb, Row, RunError};
pub use serve::{dispatch, preflight, WireResponse};
pub use value::SqlValue;

#[cfg(feature = "serve")]
pub use http::{
    serve, Context, ContextSource, HeaderView, ServeConfig, ServeError, TrustedHeaderContext,
};
