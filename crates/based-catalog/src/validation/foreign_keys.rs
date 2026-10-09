//! Validate declared foreign-key pairs and selection boundaries.
use super::{finding, has_column};
use crate::{Catalog, Selection};
use crate::{CatalogCode, CatalogDiagnostic, Table};
pub(super) fn validate(
    table: &Table,
    catalog: &Catalog,
    selection: &Selection,
) -> Vec<CatalogDiagnostic> {
    let mut findings = Vec::new();
    for foreign in &table.foreign_keys {
        if foreign.columns.is_empty()
            || foreign.columns.len() != foreign.target_columns.len()
            || foreign.columns.iter().any(|name| !has_column(table, name))
        {
            findings.push(finding(
                &table.id,
                foreign.name.as_deref(),
                CatalogCode::InvalidReference,
                "Foreign key needs matching ordered local/target column lists",
            ));
        }
        if !selection.contains(&foreign.target) {
            findings.push(finding(&table.id, foreign.name.as_deref(), CatalogCode::OutsideSelection,
                                  "Declared foreign-key target is outside selection; explicitly select it before model emission"));
            continue;
        }
        let missing_target_column = catalog
            .tables
            .iter()
            .find(|target| target.id == foreign.target)
            .is_some_and(|target| {
                foreign
                    .target_columns
                    .iter()
                    .any(|name| !has_column(target, name))
            });
        if missing_target_column {
            findings.push(finding(
                &table.id,
                foreign.name.as_deref(),
                CatalogCode::InvalidReference,
                "Foreign key names a missing target column",
            ));
        }
    }
    findings
}
