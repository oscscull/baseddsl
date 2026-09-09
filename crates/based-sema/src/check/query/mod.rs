//! Query checks: verb, return, envelope lints, distinct/for-update guards, params, clauses.
use super::*;

mod check_query;
mod verb;
mod body;
mod clauses;
mod envelope;
mod get_keyed;
mod distinct;
mod for_update;
mod param;
mod optional_params;
mod raw;

use verb::*;
use body::*;
use clauses::*;
use envelope::*;
use get_keyed::*;
use distinct::*;
use for_update::*;
use param::*;
use optional_params::*;
use raw::*;
use check_query::QueryShape;
pub(crate) use check_query::check_query;
