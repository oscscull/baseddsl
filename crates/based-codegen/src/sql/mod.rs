//! SQL generation from a `CheckedSchema`. `ddl` builds the `CREATE TABLE` script;
//! `types` maps primitives/enums/foreign keys per dialect; `generated` lowers
//! generated-column expressions; `dml`/`mutations` hold the read/write SQL.

use based_ast::{DefaultVal, Literal, Path, Predicate, Primitive, RawSpec, ShapeExpr, Value};
use based_sema::{CheckedSchema, ForeignKeys, MemberKind, RMember, RModel};

use crate::Dialect;

pub mod dml;
pub mod mutations;

mod ddl;
mod generated;
mod types;

pub(crate) use generated::*;
pub(crate) use types::*;

pub use dml::{lower_queries, LoweredQuery, ARRAY_MARK, KEYSET_PREFIX, NEST_PRESENT, NEST_SEP};
pub use mutations::{lower_mutations, LoweredMutation, LoweredWrite};

pub(crate) use ddl::{constraint_name, physical_col, raw_index_name};
pub use ddl::{ddl, ddl_with, idempotency_table_ddl, IDEMPOTENCY_TABLE};
