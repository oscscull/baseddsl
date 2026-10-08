//! Preserve ordered physical column attributes and generation/default facts.
use super::NativeType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// Catalog ordinal, never guessed from names or sorted alphabetically.
    pub position: u32,
    pub native_type: NativeType,
    pub nullable: bool,
    pub default: Option<String>,
    pub collation: Option<String>,
    pub generation: ValueGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueGeneration {
    None,
    AutoIncrement,
    Identity {
        always: bool,
    },
    Sequence {
        expression: String,
    },
    SqliteRowId {
        autoincrement: bool,
    },
    Generated {
        expression: Option<String>,
        stored: bool,
    },
}
