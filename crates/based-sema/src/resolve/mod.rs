//! Path resolution + the shared predicate/value checker.
//!
//! One expression language is used in `where`, `@scope`, named filters, and
//! relation joins, so this module is the single place paths, params,
//! filter calls, and functions are validated. Path resolution walks declared
//! fields: forward *and* backward edges are just fields, so forward traversal
//! needs no inverse and backward traversal works exactly because the inverse was
//! declared.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;

mod assign;
mod cx;
mod enums;
mod families;
mod param;
mod path;
mod predicate;
mod relation;
mod shape_expr;
mod value;

pub use assign::*;
pub use cx::*;
pub use enums::*;
pub use families::*;
pub use param::*;
pub use path::*;
pub use predicate::*;
pub use relation::*;
pub use shape_expr::*;
pub use value::*;
