//! Model resolution: AST `Model` -> `RModel`. Two phases — `skeleton` records each
//! model's fields, then `validate` resolves relation targets, inverses, decorators,
//! indexes, and sorts once every skeleton exists.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;
use crate::resolve;

mod columns;
mod decorators;
mod generated;
mod keys;
mod relations;
mod resolve_exprs;
mod skeleton;
mod validate;

pub(crate) use columns::*;
pub(crate) use decorators::*;
pub(crate) use generated::*;
pub(crate) use keys::*;
pub(crate) use relations::*;
pub(crate) use resolve_exprs::*;
pub(crate) use skeleton::*;
pub(crate) use validate::*;
