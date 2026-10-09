//! Preserve unsupported object definitions without resolving views/modules through row reads.
use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, Table, TableId, TableKind};

pub(crate) fn object(
    id: &TableId,
    kind: TableKind,
    definition: Option<String>,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Table {
    findings.push(CatalogDiagnostic { table: id.clone(), member: None, severity: CatalogSeverity::Error,
        code: CatalogCode::IncompleteMetadata,
        message: "View/virtual/shadow columns are not resolved through application-row access; native definition is retained".into() });
    Table {
        id: id.clone(),
        kind,
        native_definition: definition,
        columns: Vec::new(),
        primary_key: None,
        unique_keys: Vec::new(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
        engine: None,
        without_rowid: false,
        strict: false,
    }
}
