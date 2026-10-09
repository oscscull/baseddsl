//! Assemble one model from checked mappings; formatting is delegated to based-fmt.
use super::{
    defaults, generation, identifier::quote, indexes, relations, types, ImportReport, ModelNames,
};
use based_catalog::{Catalog, CatalogDialect, Table};
use based_manifest::Manifest;
use std::collections::BTreeMap;

pub(crate) fn render(
    catalog: &Catalog,
    table: &Table,
    names: &BTreeMap<based_catalog::TableId, ModelNames>,
    manifest: &Manifest,
    report: &mut ImportReport,
) -> String {
    let naming = &names[&table.id];
    let dialect = catalog.source.dialect;
    let relations = relations::plan(catalog, table, report);
    let mut source = format!("@table({})\n", quote(&table.id.name));
    if dialect != CatalogDialect::Sqlite {
        source.push_str(&format!("@schema({})\n", quote(&table.id.namespace)));
    }
    match &table.primary_key {
        None => source.push_str("@no_id(\"imported keyless table\")\n"),
        Some(key)
            if !table
                .columns
                .iter()
                .any(|column| naming.fields[&column.name] == "id") =>
        {
            source.push_str(&format!(
                "@key({})\n",
                indexes::fields(&key.columns, naming)
            ));
        }
        Some(_) => {}
    }
    if let Some(key) = &table.primary_key {
        if key.name.is_some() {
            report.loss(&table.id, key.name.as_deref(), "Primary-key column order is preserved; its native constraint name is not represented by BSL");
        }
    }
    if table.strict || table.without_rowid {
        report.loss(&table.id, None, "SQLite STRICT/WITHOUT ROWID storage semantics are retained in the catalog but not reproduced by generated BSL DDL");
    }
    source.push_str(&format!("{} {{\n", naming.model));
    for column in &table.columns {
        let field = &naming.fields[&column.name];
        let serial = generation::serial(column, table, dialect, report);
        let ty = types::scalar(column, &table.id, dialect, report);
        if column.native_type.charset.is_some()
            || column
                .collation
                .as_deref()
                .is_some_and(|collation| collation != "BINARY" && collation != "pg_catalog.default")
        {
            report.loss(&table.id, Some(&column.name), "Native character set/collation remains in the catalog; BSL has no general column modifier to reproduce it in future DDL");
        }
        let Some(ty) = ty else {
            continue;
        };
        let optional = if column.nullable { "?" } else { "" };
        if let Some(foreign) = relations.get(&column.name) {
            let reason = if manifest.schema.foreign_keys == "none" {
                "\"imported declared foreign key\", "
            } else {
                ""
            };
            source.push_str(&format!(
                "{field}: {}{optional} (column {}) @fk({reason}on_delete: {}, on_update: {})\n",
                names[&foreign.target].model,
                quote(&column.name),
                relations::action(foreign.on_delete),
                relations::action(foreign.on_update)
            ));
            continue;
        }
        let ty = if serial { "serial".into() } else { ty };
        source.push_str(&format!(
            "{field}: {ty}{optional} (column {})",
            quote(&column.name)
        ));
        if let Some(default) = defaults::render(column, &table.id, dialect, &ty, report) {
            source.push_str(&format!(" (default {default})"));
        }
        source.push('\n');
    }
    for index in indexes::render(table, naming, report) {
        source.push_str(&format!("{index}\n"));
    }
    source.push_str("}\n");
    source
}
