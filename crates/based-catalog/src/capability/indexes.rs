//! Identify index forms whose complete semantics need manual raw-index authoring.
use crate::validation::finding;
use crate::{CatalogCode, CatalogDiagnostic, IndexDirection, IndexTarget, Table};

pub fn assess(table: &Table) -> Vec<CatalogDiagnostic> {
    table.indexes.iter().filter(|index| !index.valid || index.predicate.is_some()
        || !index.included_columns.is_empty() || index.parts.iter().any(|part|
            matches!(part.target, IndexTarget::Expression(_)) || part.prefix_length.is_some()
            || part.direction == IndexDirection::Descending))
        .map(|index| finding(&table.id, Some(&index.name), CatalogCode::UnsupportedAttribute,
                             "Special/invalid index definition is retained; automatic import requires manual raw-index review"))
        .collect()
}
