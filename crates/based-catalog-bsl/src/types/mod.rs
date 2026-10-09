//! Report verified dialect mapping and native-definition losses separately.
mod decimal;
mod mariadb;
mod postgres;
mod sqlite;
use super::ImportReport;
use based_catalog::{CatalogCode, CatalogDialect, Column, TableId};

pub(crate) fn scalar(
    column: &Column,
    table: &TableId,
    dialect: CatalogDialect,
    report: &mut ImportReport,
) -> Option<String> {
    let native = &column.native_type;
    let declaration = native.declaration.trim().to_ascii_lowercase();
    let mapped = match dialect {
        CatalogDialect::Sqlite => sqlite::map(native, &declaration),
        CatalogDialect::Postgres => postgres::map(native, &declaration),
        CatalogDialect::MariaDb => mariadb::map(native, &declaration),
    };
    let Some((mapped, exact)) = mapped else {
        report.error(
            table,
            Some(&column.name),
            CatalogCode::UnsupportedType,
            "Native type has no verified BSL mapping; retain its declaration and author manually",
        );
        return None;
    };
    if !exact {
        report.loss(table, Some(&column.name), format!("BSL {mapped} does not reproduce native declaration {:?}; review native bounds/precision/storage before any future DDL", native.declaration));
    }
    Some(mapped)
}

fn base(declaration: &str) -> &str {
    declaration.split(['(', ' ']).next().unwrap_or_default()
}
