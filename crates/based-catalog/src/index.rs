//! Physical index definitions; opaque expressions/predicates are retained, never flattened.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexOrigin {
    Explicit,
    PrimaryKey,
    UniqueConstraint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexTarget {
    Column(String),
    Expression(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPart {
    pub target: IndexTarget,
    pub direction: IndexDirection,
    pub prefix_length: Option<u32>,
    pub collation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    pub origin: IndexOrigin,
    pub unique: bool,
    pub parts: Vec<IndexPart>,
    pub included_columns: Vec<String>,
    pub method: Option<String>,
    pub predicate: Option<String>,
    pub native_definition: Option<String>,
    pub valid: bool,
}
