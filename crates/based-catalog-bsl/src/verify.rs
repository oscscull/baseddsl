//! Compare compiler-resolved physical columns/keys/FKs with independent catalog facts.
use super::{ImportReport, ModelNames};
use based_catalog::{Catalog, CatalogCode, CatalogDialect};
use based_sema::{CheckedSchema, ForeignKeys, MemberKind};
use std::collections::BTreeMap;

pub(crate) fn physical(
    catalog: &Catalog,
    schema: &CheckedSchema,
    names: &BTreeMap<based_catalog::TableId, ModelNames>,
    foreign_keys: ForeignKeys,
    report: &mut ImportReport,
) {
    for table in &catalog.tables {
        let Some(model) = schema.model(&names[&table.id].model) else {
            report.error(
                &table.id,
                None,
                CatalogCode::IncompleteMetadata,
                "Compiler output is missing a selected model",
            );
            continue;
        };
        let namespace = match catalog.source.dialect {
            CatalogDialect::Sqlite => "main",
            _ => model.schema.as_deref().unwrap_or_default(),
        };
        let actual: BTreeMap<_, _> = model
            .members
            .iter()
            .map(|member| {
                let nullable = match &member.kind {
                    MemberKind::Scalar { optional, .. } | MemberKind::Forward { optional, .. } => {
                        *optional
                    }
                    MemberKind::Inverse { .. } => false,
                };
                (member.physical_col().to_owned(), nullable)
            })
            .collect();
        let expected: BTreeMap<_, _> = table
            .columns
            .iter()
            .map(|column| (column.name.clone(), column.nullable))
            .collect();
        let primary = table
            .primary_key
            .as_ref()
            .map_or_else(Vec::new, |key| key.columns.clone());
        if model.table != table.id.name
            || namespace != table.id.namespace
            || actual != expected
            || model.pk_columns() != primary
        {
            report.error(&table.id, None, CatalogCode::InvalidReference, "Compiler-resolved table/column/nullability/primary-key mapping differs from the catalog");
        }
        if model.scope.is_some()
            || model.soft_delete.is_some()
            || model.created.is_some()
            || model.updated.is_some()
        {
            report.error(
                &table.id,
                None,
                CatalogCode::UnsupportedAttribute,
                "Compiler output introduced application policy absent from the catalog",
            );
        }
        super::verify_indexes::compare(table, schema, model, report);
        super::verify_foreign::compare(table, schema, model, foreign_keys, report);
    }
}
