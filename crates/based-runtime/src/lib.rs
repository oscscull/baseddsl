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
//! The concrete MariaDB driver + HTTP server are the *next* slice; the [`run::Db`]
//! trait is the seam (a [`run::MockDb`] stands in for tests), exactly mirroring the
//! generated client's abstract `Transport`. The write path (mutations: engine
//! id-gen, `tx`, write-response) is likewise the next slice.

pub mod load;
pub mod plan;
pub mod run;
pub mod scan;
pub mod value;

pub use load::Compiled;
pub use plan::{plan_query, Envelope, PlanError, QueryPlan, Request, Stmt};
pub use run::{run_query, Db, MockDb, Row};
pub use value::SqlValue;
