//! Read-side SQL: a `query` lowers to a parameterized SELECT.
//!
//! The soft-delete tombstone and `@scope` are injected into every SELECT — the root
//! `WHERE` and each joined `ON` — as compiler primitives. Placeholders are `:name`
//! (`$ctx.org` -> `:ctx_org`), bound by the runtime; text branches on [`crate::Dialect`].
use std::collections::{HashMap, HashSet};

pub(crate) use based_ast::*;
use based_sema::{
    CheckedSchema, EnumValue, MemberKind, REnum, RMember, RModel, RQuery, ScopeInject, SoftDelete,
    SoftMode,
};

use crate::Dialect;

mod aggregate;
mod columns;
mod computed;
mod count;
mod expr;
mod filter;
mod join_on;
mod joins;
mod keyset;
mod literals;
mod lock;
mod lower;
mod nest;
mod ops;
mod order;
mod outputs;
mod path;
mod project;
mod raw;
mod relation;
mod render;
mod resolve;
mod scope;
mod select;
mod shape;
mod soft_delete;
mod to_many;
mod types;
mod value;
mod wire;

pub(crate) use aggregate::*;
pub(crate) use columns::*;
pub(crate) use computed::*;
pub(crate) use count::*;
pub(crate) use join_on::*;
pub(crate) use joins::*;
pub(crate) use keyset::*;
pub(crate) use literals::*;
pub(crate) use lock::*;
pub(crate) use ops::*;
pub(crate) use order::*;
pub(crate) use outputs::*;
pub(crate) use path::*;
pub(crate) use project::*;
pub(crate) use raw::*;
pub(crate) use relation::*;
pub(crate) use select::*;
pub(crate) use shape::*;
pub(crate) use soft_delete::*;
pub(crate) use types::*;

pub(crate) use filter::build_wheres;
pub(crate) use lower::{index_queries, lower_query};
pub(crate) use value::{bref_name, param_key};

pub use lower::{lower_queries, LoweredQuery};
pub use render::dml;
pub use wire::{ARRAY_MARK, KEYSET_PREFIX, NEST_PRESENT, NEST_SEP};
