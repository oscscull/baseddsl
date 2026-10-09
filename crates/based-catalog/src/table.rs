//! A selected physical object and its catalog-owned schema facts.
use super::{CheckConstraint, Column, ForeignKey, Index, Key, TableId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    VirtualTable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub id: TableId,
    pub kind: TableKind,
    pub columns: Vec<Column>,
    pub primary_key: Option<Key>,
    pub unique_keys: Vec<Key>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
    pub checks: Vec<CheckConstraint>,
    pub native_definition: Option<String>,
    pub engine: Option<String>,
    pub without_rowid: bool,
    pub strict: bool,
}
