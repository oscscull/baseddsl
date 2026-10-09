//! Name generation-definition loss without reading or advancing sequence state.
use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId, ValueGeneration};

pub(crate) fn report(
    generation: &ValueGeneration,
    id: &TableId,
    name: &str,
    findings: &mut Vec<CatalogDiagnostic>,
) {
    if !matches!(
        generation,
        ValueGeneration::Identity { .. } | ValueGeneration::Sequence { .. }
    ) {
        return;
    }
    findings.push(CatalogDiagnostic { table: id.clone(), member: Some(name.into()), severity: CatalogSeverity::Warning,
        code: CatalogCode::DefinitionLoss,
        message: "Native sequence options/state are not normalized; retain existing database generation and review future migration DDL".into() });
}
