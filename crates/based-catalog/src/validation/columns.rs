//! Validate physical column coverage and identity.
use super::finding;
use crate::{CatalogCode, CatalogDiagnostic, Table};
use std::collections::BTreeSet;
pub(super) fn validate(table: &Table) -> Vec<CatalogDiagnostic> {
    let mut names = BTreeSet::new();
    let mut positions = BTreeSet::new();
    let mut findings = Vec::new();
    if table.columns.is_empty() {
        findings.push(finding(
            &table.id,
            None,
            CatalogCode::IncompleteMetadata,
            "No physical column metadata was returned",
        ));
    }
    for column in &table.columns {
        if !names.insert(&column.name) || !positions.insert(column.position) {
            findings.push(finding(
                &table.id,
                Some(&column.name),
                CatalogCode::DuplicateFact,
                "Duplicate column name or ordinal",
            ));
        }
        if column.name.is_empty() || column.native_type.declaration.is_empty() {
            findings.push(finding(
                &table.id,
                Some(&column.name),
                CatalogCode::IncompleteMetadata,
                "Column name and native declaration are required",
            ));
        }
    }
    findings
}
