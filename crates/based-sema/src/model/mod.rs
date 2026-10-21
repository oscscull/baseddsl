//! Model resolution: AST `Model` -> `RModel`. Two phases — `skeleton` records each
//! model's fields, then `validate` resolves relation targets, inverses, decorators,
//! indexes, and sorts once every skeleton exists.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;
use crate::resolve;

mod column_aliases;
mod columns;
mod decimal_fit;
mod decorators;
mod generated;
mod keys;
mod relations;
mod resolve_exprs;
mod skeleton;
mod soft_overrides;
mod validate;

pub(crate) use column_aliases::*;
pub(crate) use columns::*;
use decimal_fit::decimal_fits;
pub(crate) use decorators::*;
pub(crate) use generated::*;
pub(crate) use keys::*;
pub(crate) use relations::*;
pub(crate) use resolve_exprs::*;
pub(crate) use skeleton::*;
pub(crate) use soft_overrides::*;
pub(crate) use validate::*;
