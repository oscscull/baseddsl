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
//! The concrete MariaDB driver + HTTP server are the *next* slice; the [`run::Db`]
//! trait is the seam (a [`run::MockDb`] stands in for tests), exactly mirroring the
//! generated client's abstract `Transport`. The write response is the created row's
//! engine `id` today — the declared-shape re-select (RETURNING) is deferred (D12).

pub mod id;
pub mod load;
pub mod plan;
pub mod run;
pub mod scan;
pub mod value;

pub use id::{IdGen, SeqIdGen};
pub use load::Compiled;
pub use plan::{
    plan_mutation, plan_query, Envelope, MutationPlan, PlanError, QueryPlan, Request, Stmt,
};
pub use run::{run_mutation, run_query, Db, MockDb, Row};
pub use value::SqlValue;
