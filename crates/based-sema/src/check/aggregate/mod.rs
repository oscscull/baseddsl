//! Aggregate shape + query rules: agg-call validation, `group by` / `having` consistency.
use super::*;

mod agg_call;
mod agg_query;
mod bucket_call;
mod having;
mod is_agg_shape;

pub(crate) use agg_call::*;
pub(crate) use agg_query::*;
pub(crate) use bucket_call::*;
use having::*;
pub(crate) use is_agg_shape::*;
