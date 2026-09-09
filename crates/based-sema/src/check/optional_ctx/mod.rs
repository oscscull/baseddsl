//! Optional `$ctx.<field>?` placement: a read-filter-only construct.
//! Absent-means-widen is safe in a query `where`, but would silently unfilter a scope or a
//! write — so it is rejected in a scope term, a mutation, or a named filter, and only ever
//! on a `$ctx.<field>`.
use super::*;

mod query_reads;
mod reject;

use reject::*;
pub(crate) use query_reads::check_optional_ctx_query;
pub(crate) use reject::{forbid_optional_ctx_writes, forbid_optional_in_pred};
