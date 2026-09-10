//! Shape / query / mutation / filter checks, and the four query inferences (verb, param
//! type, filter, target model).

use based_ast::*;

use crate::ir::*;
use crate::resolve::{self, Cx};

mod aggregate;
mod check_filter;
mod is_key_link;
mod join_path;
mod mutation;
mod optional_ctx;
mod query;
mod resolved_return;
mod shape;
mod unknown_field;

pub(crate) use aggregate::*;
pub(crate) use check_filter::*;
pub use is_key_link::is_key_link;
pub(crate) use join_path::*;
pub(crate) use mutation::*;
pub(crate) use optional_ctx::*;
pub(crate) use query::*;
pub(crate) use resolved_return::*;
pub(crate) use shape::*;
pub(crate) use unknown_field::*;
