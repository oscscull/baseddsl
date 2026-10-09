//! Only declared foreign-key metadata, including referential semantics and pair order.
use super::{Deferral, TableId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferentialAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchMode {
    Simple,
    Full,
    Partial,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub target: TableId,
    pub target_columns: Vec<String>,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
    pub match_mode: MatchMode,
    pub deferral: Deferral,
}
