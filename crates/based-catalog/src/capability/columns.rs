//! Surface unsupported native column semantics instead of silently mapping them away.
use crate::validation::finding;
use crate::{CatalogCode, CatalogDiagnostic, Table, TypeFamily, ValueGeneration};

pub fn assess(table: &Table) -> Vec<CatalogDiagnostic> {
    table
        .columns
        .iter()
        .flat_map(|column| {
            let mut issues = Vec::new();
            if matches!(
                column.native_type.family,
                TypeFamily::Other | TypeFamily::Array | TypeFamily::Domain | TypeFamily::Enum
            ) {
                issues.push(finding(
                    &table.id,
                    Some(&column.name),
                    CatalogCode::UnsupportedType,
                    "Native type identity is retained but requires explicit manual BSL mapping",
                ));
            }
            if column.native_type.unsigned {
                issues.push(finding(
                    &table.id,
                    Some(&column.name),
                    CatalogCode::UnsupportedAttribute,
                    "Unsigned range is not represented by signed BSL int",
                ));
            }
            if matches!(column.generation, ValueGeneration::Generated { .. }) {
                issues.push(finding(
                    &table.id,
                    Some(&column.name),
                    CatalogCode::UnsupportedAttribute,
                    "Native generated expression requires manual BSL expression review",
                ));
            }
            issues
        })
        .collect()
}
