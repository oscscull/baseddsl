//! Reject automatic view/check/deferrable-key conversion without discarding their facts.
use crate::validation::finding;
use crate::{CatalogCode, CatalogDiagnostic, Deferral, Table, TableKind};

pub fn assess(table: &Table) -> Vec<CatalogDiagnostic> {
    let mut issues = Vec::new();
    if table.kind != TableKind::Table {
        issues.push(finding(
            &table.id,
            None,
            CatalogCode::UnsupportedObject,
            "View/materialized/virtual definitions are not ordinary importable table models",
        ));
    }
    if !table.checks.is_empty() {
        issues.push(finding(
            &table.id,
            None,
            CatalogCode::UnsupportedAttribute,
            "Declared CHECK expressions are retained and require manual representation",
        ));
    }
    issues.extend(
        table
            .primary_key
            .iter()
            .chain(&table.unique_keys)
            .filter(|key| key.deferral != Deferral::NotDeferrable)
            .map(|key| {
                finding(
                    &table.id,
                    key.name.as_deref(),
                    CatalogCode::UnsupportedAttribute,
                    "Deferrable key semantics are not automatically represented",
                )
            }),
    );
    issues
}
