//! Project conventions and compiler alias representability.
use super::ImportReport;
use based_catalog::{Catalog, CatalogCode, CatalogDialect};
use based_manifest::Manifest;

pub(crate) fn check(catalog: &Catalog, manifest: &Manifest, report: &mut ImportReport) {
    let dialect = match catalog.source.dialect {
        CatalogDialect::MariaDb => "mariadb",
        CatalogDialect::Postgres => "postgres",
        CatalogDialect::Sqlite => "sqlite",
    };
    for table in &catalog.tables {
        if manifest.dialect != dialect
            || !matches!(manifest.schema.foreign_keys.as_str(), "none" | "all")
        {
            report.error(
                &table.id,
                None,
                CatalogCode::UnsupportedAttribute,
                "Project dialect/FK convention is incompatible with the catalog",
            );
        }
        if table.id.name.contains('.')
            || (catalog.source.dialect != CatalogDialect::Sqlite
                && table.id.namespace.chars().any(char::is_whitespace))
            || (catalog.source.dialect == CatalogDialect::Sqlite && table.id.namespace != "main")
        {
            report.error(
                &table.id,
                None,
                CatalogCode::UnsupportedAttribute,
                "Physical table/namespace alias cannot be represented by the current compiler",
            );
        }
    }
}
