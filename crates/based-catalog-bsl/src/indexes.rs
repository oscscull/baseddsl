//! Preserve declared key/index part order; constraint-backed indexes are not duplicated.
use super::{ImportReport, ModelNames};
use based_catalog::{CatalogCode, IndexOrigin, IndexTarget, Table};

pub(crate) fn render(table: &Table, names: &ModelNames, report: &mut ImportReport) -> Vec<String> {
    let mut indexes = Vec::new();
    for key in &table.unique_keys {
        indexes.push(format!("@index ({}) unique", fields(&key.columns, names)));
        report.loss(&table.id, key.name.as_deref(), "Unique-key name and constraint-versus-index DDL form are not reproduced; ordered uniqueness is preserved");
    }
    for index in &table.indexes {
        let mut columns = Vec::new();
        for part in &index.parts {
            let IndexTarget::Column(name) = &part.target else {
                continue;
            };
            columns.push(name.clone());
            let original = table
                .columns
                .iter()
                .find(|column| column.name == *name)
                .and_then(|column| column.collation.as_ref());
            if part
                .collation
                .as_ref()
                .is_some_and(|collation| Some(collation) != original)
            {
                report.error(
                    &table.id,
                    Some(&index.name),
                    CatalogCode::UnsupportedAttribute,
                    "Index collation differs from its column and has no ordinary BSL mapping",
                );
            }
        }
        if index
            .method
            .as_deref()
            .is_some_and(|method| !method.eq_ignore_ascii_case("btree"))
        {
            report.error(
                &table.id,
                Some(&index.name),
                CatalogCode::UnsupportedAttribute,
                "Native access method requires manual index representation",
            );
        }
        if index.origin != IndexOrigin::Explicit {
            let matched = index.unique
                && match index.origin {
                    IndexOrigin::PrimaryKey => table
                        .primary_key
                        .as_ref()
                        .is_some_and(|key| key.columns == columns),
                    IndexOrigin::UniqueConstraint => {
                        table.unique_keys.iter().any(|key| key.columns == columns)
                    }
                    IndexOrigin::Explicit => unreachable!(),
                };
            if !matched {
                report.error(
                    &table.id,
                    Some(&index.name),
                    CatalogCode::IncompleteMetadata,
                    "Constraint-backed index has no matching declared ordered key",
                );
            }
            continue;
        }
        indexes.push(format!(
            "@index ({}){}",
            fields(&columns, names),
            if index.unique { " unique" } else { "" }
        ));
        report.loss(&table.id, Some(&index.name), "Ordered index columns/uniqueness are preserved; BSL generates its own index name for future DDL");
    }
    indexes
}

pub(crate) fn fields(columns: &[String], names: &ModelNames) -> String {
    columns
        .iter()
        .map(|column| names.fields[column].as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
