//! Validate declared primary and unique key column lists.
use super::{finding, has_column};
use crate::Key;
use crate::{CatalogCode, CatalogDiagnostic, Table};
use std::collections::BTreeSet;
pub(super) fn validate(table: &Table) -> Vec<CatalogDiagnostic> {
    table
        .primary_key
        .iter()
        .chain(&table.unique_keys)
        .filter(|key| invalid_key(table, key))
        .map(|key| {
            finding(
                &table.id,
                key.name.as_deref(),
                CatalogCode::InvalidReference,
                "A key needs distinct ordered columns belonging to its table",
            )
        })
        .collect()
}

fn invalid_key(table: &Table, key: &Key) -> bool {
    key.columns.is_empty()
        || key.columns.iter().collect::<BTreeSet<_>>().len() != key.columns.len()
        || key.columns.iter().any(|name| !has_column(table, name))
}
