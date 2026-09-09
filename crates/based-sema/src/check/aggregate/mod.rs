//! Aggregate shape + query rules: agg-call validation, `group by` / `having` consistency.
use super::*;

mod is_agg_shape;
mod agg_call;
mod agg_query;
mod having;

use having::*;
pub(crate) use is_agg_shape::*;
pub(crate) use agg_call::*;
pub(crate) use agg_query::*;
