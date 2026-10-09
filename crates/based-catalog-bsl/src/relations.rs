//! Only declared, representable FK pairs become forward fields.
use super::{types, ImportReport};
use based_catalog::{Catalog, CatalogCode, Column, ForeignKey, Table, TableId, ValueGeneration};
use std::collections::BTreeMap;

pub(crate) fn plan<'a>(
    catalog: &'a Catalog,
    table: &'a Table,
    report: &mut ImportReport,
) -> BTreeMap<String, &'a ForeignKey> {
    let mut relations = BTreeMap::new();
    for foreign in &table.foreign_keys {
        let Some(target) = catalog
            .tables
            .iter()
            .find(|target| target.id == foreign.target)
        else {
            continue;
        };
        if foreign.columns.len() != 1
            || target
                .primary_key
                .as_ref()
                .is_none_or(|key| key.columns != foreign.target_columns)
        {
            report.error(&table.id, foreign.name.as_deref(), CatalogCode::UnsupportedAttribute, "A BSL forward edge must preserve its target primary key; arbitrary composite FK aliases and references to alternate unique keys require manual representation");
            continue;
        }
        let Some(local) = table
            .columns
            .iter()
            .find(|column| column.name == foreign.columns[0])
        else {
            continue;
        };
        let Some(remote) = target
            .columns
            .iter()
            .find(|column| column.name == foreign.target_columns[0])
        else {
            continue;
        };
        if !compatible(local, remote, &table.id, catalog, report)
            || local.default.is_some()
            || local.generation != ValueGeneration::None
        {
            report.error(&table.id, Some(&local.name), CatalogCode::UnsupportedAttribute, "Relation would lose native type/default/generation semantics; retain it for manual representation");
            continue;
        }
        if relations.insert(local.name.clone(), foreign).is_some() {
            report.error(
                &table.id,
                Some(&local.name),
                CatalogCode::UnsupportedAttribute,
                "Overlapping declared foreign keys cannot occupy one BSL field",
            );
        }
        if foreign.name.is_some() {
            report.loss(&table.id, foreign.name.as_deref(), "BSL preserves FK pairs/actions but generates its own constraint name for future DDL");
        }
    }
    relations
}

fn compatible(
    local: &Column,
    remote: &Column,
    table: &TableId,
    catalog: &Catalog,
    report: &mut ImportReport,
) -> bool {
    let dialect = catalog.source.dialect;
    let local = types::scalar(local, table, dialect, report);
    let remote = types::scalar(remote, table, dialect, report);
    local.is_some() && local == remote
}

pub(crate) fn action(action: based_catalog::ReferentialAction) -> &'static str {
    use based_catalog::ReferentialAction;
    match action {
        ReferentialAction::NoAction => "no_action",
        ReferentialAction::Restrict => "restrict",
        ReferentialAction::Cascade => "cascade",
        ReferentialAction::SetNull => "set_null",
        ReferentialAction::SetDefault => "set_default", // shared screen blocks this before formatting
    }
}
