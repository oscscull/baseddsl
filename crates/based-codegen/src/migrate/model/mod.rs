//! Neutral snapshot model: the diff baseline and its `schema.snap` text form.
//!
//! The dialect-neutral [`Snapshot`] type (tables, columns, indexes, scope decls), built
//! from a resolved `CheckedSchema` ([`Snapshot::from_schema`]), rendered to the canonical
//! `schema.snap` text ([`Snapshot::render`]), and parsed back ([`Snapshot::parse`]) so a
//! stored baseline round-trips for the drift check. Names no dialect — SQL lives in
//! [`super::sql`].

mod from_schema;
mod ir;
mod parse;
mod render;

pub use from_schema::{foreign_key_snaps, target_pk_column};
pub(crate) use from_schema::index_name;
pub use ir::{
    ColumnSnap, ForeignKeySnap, IndexSnap, Rename, ScopeDeclSnap, ScopeTermSnap, Snapshot,
    TableSnap,
};
pub use parse::ParseError;
pub use render::{fk_spec_text, snapshot};
pub(crate) use render::{col_list_text, index_spec_text, render_scope_decl};
