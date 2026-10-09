//! Generation strategy compatibility is distinct from native scalar storage.
use super::ImportReport;
use based_catalog::{CatalogCode, CatalogDialect, Column, Table, ValueGeneration};

pub(crate) fn serial(
    column: &Column,
    table: &Table,
    dialect: CatalogDialect,
    report: &mut ImportReport,
) -> bool {
    if column.generation == ValueGeneration::None {
        return false;
    }
    let sole_primary = table
        .primary_key
        .as_ref()
        .is_some_and(|key| key.columns == [column.name.clone()]);
    let supported = match (&column.generation, dialect) {
        (
            ValueGeneration::SqliteRowId {
                autoincrement: true,
            },
            CatalogDialect::Sqlite,
        ) => true,
        (ValueGeneration::AutoIncrement, CatalogDialect::MariaDb) => column
            .native_type
            .declaration
            .to_ascii_lowercase()
            .starts_with("bigint"),
        (ValueGeneration::Identity { always: true }, CatalogDialect::Postgres) => column
            .native_type
            .declaration
            .eq_ignore_ascii_case("bigint"),
        _ => false,
    };
    if !sole_primary || !supported || column.nullable || column.default.is_some() {
        report.error(&table.id, Some(&column.name), CatalogCode::UnsupportedAttribute, "Native generation cannot be reproduced by a sole serial key (rowid reuse, sequence, BY DEFAULT, composite/non-key generation and generated expressions require manual work)");
        return false;
    }
    true
}
