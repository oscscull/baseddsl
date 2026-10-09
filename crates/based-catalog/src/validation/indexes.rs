//! Validate index column references without interpreting opaque expressions.
use super::{finding, has_column};
use crate::IndexTarget;
use crate::{CatalogCode, CatalogDiagnostic, Table};
pub(super) fn validate(table: &Table) -> Vec<CatalogDiagnostic> {
    let mut findings = Vec::new();
    for index in &table.indexes {
        let missing = index.parts.iter().any(
            |part| matches!(&part.target, IndexTarget::Column(name) if !has_column(table, name)),
        ) || index
            .included_columns
            .iter()
            .any(|name| !has_column(table, name));
        if index.parts.is_empty() || missing {
            findings.push(finding(
                &table.id,
                Some(&index.name),
                CatalogCode::InvalidReference,
                "Index needs parts and its referenced columns must exist",
            ));
        }
    }
    findings
}
