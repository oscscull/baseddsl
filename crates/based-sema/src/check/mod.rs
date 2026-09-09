//! Shape / query / mutation / filter checks, and the four query inferences (verb, param
//! type, filter, target model).

use based_ast::*;

use crate::ir::*;
use crate::resolve::{self, Cx};

mod is_key_link;
mod unknown_field;
mod join_path;
mod resolved_return;
mod check_filter;
mod shape;
mod aggregate;
mod optional_ctx;
mod query;
mod mutation;

pub(crate) use unknown_field::*;
pub(crate) use join_path::*;
pub(crate) use resolved_return::*;
pub(crate) use check_filter::*;
pub(crate) use shape::*;
pub(crate) use aggregate::*;
pub(crate) use optional_ctx::*;
pub(crate) use query::*;
pub(crate) use mutation::*;
pub use is_key_link::is_key_link;
