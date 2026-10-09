//! Native type identity and declared modifiers; no application-data type inference.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeFamily {
    Boolean,
    Integer,
    Real,
    Decimal,
    Text,
    Binary,
    Uuid,
    Date,
    Time,
    Timestamp,
    Json,
    Enum,
    Domain,
    Array,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqliteAffinity {
    Integer,
    Real,
    Text,
    Blob,
    Numeric,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeType {
    /// Full catalog declaration, not a lossy BSL primitive spelling.
    pub declaration: String,
    pub family: TypeFamily,
    pub unsigned: bool,
    pub length: Option<u64>,
    pub precision: Option<u16>,
    pub scale: Option<i16>,
    pub timezone: Option<bool>,
    pub charset: Option<String>,
    /// Qualified enum/domain identity where the server exposes one.
    pub named_type: Option<super::QualifiedName>,
    pub enum_values: Vec<String>,
    pub sqlite_affinity: Option<SqliteAffinity>,
}

impl NativeType {
    pub fn declared(declaration: impl Into<String>, family: TypeFamily) -> Self {
        Self {
            declaration: declaration.into(),
            family,
            unsigned: false,
            length: None,
            precision: None,
            scale: None,
            timezone: None,
            charset: None,
            named_type: None,
            enum_values: Vec::new(),
            sqlite_affinity: None,
        }
    }
}
