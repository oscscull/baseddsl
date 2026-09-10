//! SQL write-side lowering: a `mutation` body lowers to INSERT / UPDATE / DELETE
//! statements plus a declared-shape re-select. A `delete` on a `@soft_delete` model
//! becomes a tombstone UPDATE (`restore` its inverse, `hard delete` the explicit real
//! DELETE); the soft-delete live predicate and `@scope` are injected into every write's
//! WHERE, and the whole body runs in one engine-owned transaction.

pub(crate) use based_ast::*;
pub(crate) use based_sema::{
    CheckedSchema, MemberKind, PkStrategy, RMember, RModel, RMutation, ScopeInject, SoftDelete,
    SoftMode,
};

pub(crate) use crate::sql::dml::{
    bref_name, json_output_paths, physical_col, project_return, push_joins, render_raw, soft_pred,
    BackCtx, Select,
};
pub(crate) use crate::Dialect;

mod bindings;
mod bulk;
mod create;
mod cx;
mod delete;
mod emit;
mod ir;
mod lower;
mod readback;
mod restore;
mod stmt;
mod update;
mod upsert;

pub use emit::mutations;
pub use ir::*;
pub use lower::lower_mutations;

pub(crate) use bindings::collect_binding_refs;
pub(crate) use bulk::lower_bulk_create;
pub(crate) use create::{lower_create, serial_return_col};
pub(crate) use cx::LowerCx;
pub(crate) use delete::lower_delete;
pub(crate) use lower::flat_writes;
pub(crate) use readback::{bulk_readback, ret_select};
pub(crate) use restore::lower_restore;
pub(crate) use stmt::{
    delete_stmt, inject_guards, push_where, set_lhs, timestamp_cols, tombstone_set, update_stmt,
    updated_bump,
};
pub(crate) use update::lower_update;
pub(crate) use upsert::{conflict_update_sets, upsert_tail, upsert_tail_and_key};
