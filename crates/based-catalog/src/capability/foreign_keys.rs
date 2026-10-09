//! Surface referential semantics absent from the automatic BSL relation profile.
use crate::validation::finding;
use crate::{CatalogCode, CatalogDiagnostic, Deferral, MatchMode, ReferentialAction, Table};

pub fn assess(table: &Table) -> Vec<CatalogDiagnostic> {
    table
        .foreign_keys
        .iter()
        .filter(|foreign| {
            foreign.deferral != Deferral::NotDeferrable
                || foreign.match_mode != MatchMode::Simple
                || foreign.on_delete == ReferentialAction::SetDefault
                || foreign.on_update == ReferentialAction::SetDefault
        })
        .map(|foreign| {
            finding(
                &table.id,
                foreign.name.as_deref(),
                CatalogCode::UnsupportedAttribute,
                "Unknown or nonstandard FK match/deferral/SET DEFAULT semantics need manual review",
            )
        })
        .collect()
}
