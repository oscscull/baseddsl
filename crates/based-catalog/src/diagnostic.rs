//! Catalog/capability findings refer to physical objects, not SQL connection secrets.
use super::TableId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CatalogSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CatalogCode {
    MissingTable,
    UnexpectedTable,
    DuplicateFact,
    InvalidReference,
    IncompleteMetadata,
    OutsideSelection,
    UnsupportedObject,
    UnsupportedType,
    UnsupportedAttribute,
    DefinitionLoss,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CatalogDiagnostic {
    pub severity: CatalogSeverity,
    pub code: CatalogCode,
    pub table: TableId,
    pub member: Option<String>,
    pub message: String,
}
