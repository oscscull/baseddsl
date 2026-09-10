//! Query checks: verb, return, envelope lints, distinct/for-update guards, params, clauses.
use super::*;

mod body;
mod check_query;
mod clauses;
mod distinct;
mod envelope;
mod for_update;
mod get_keyed;
mod optional_params;
mod param;
mod raw;
mod verb;

use body::*;
pub(crate) use check_query::check_query;
use check_query::QueryShape;
use clauses::*;
use distinct::*;
use envelope::*;
use for_update::*;
use get_keyed::*;
use optional_params::*;
use param::*;
use raw::*;
use verb::*;
