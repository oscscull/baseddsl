//! Only declared, representable FK pairs become forward fields.
use super::{identifier::quote, types, ImportReport, ModelNames};
use based_catalog::{Catalog, CatalogCode, Column, ForeignKey, Table, TableId, ValueGeneration};
use std::collections::{BTreeMap, BTreeSet};

/// Representable declared FKs, keyed by their first local column in table order. A
/// composite FK becomes one forward field over all its local columns.
pub(crate) fn plan<'a>(
    catalog: &'a Catalog,
    table: &'a Table,
    report: &mut ImportReport,
) -> BTreeMap<String, &'a ForeignKey> {
    let mut relations = BTreeMap::new();
    let mut occupied = BTreeSet::new();
    for foreign in &table.foreign_keys {
        let Some(target) = catalog
            .tables
            .iter()
            .find(|target| target.id == foreign.target)
        else {
            continue;
        };
        if target
            .primary_key
            .as_ref()
            .is_none_or(|key| key.columns != foreign.target_columns)
        {
            report.error(&table.id, foreign.name.as_deref(), CatalogCode::UnsupportedAttribute, "A BSL forward edge must reference its target primary key in key order; references to alternate unique keys or reordered key parts require manual representation");
            continue;
        }
        let Some(locals) = representable_pairs(catalog, table, target, foreign, report) else {
            continue;
        };
        if locals.len() > 1 {
            if locals
                .iter()
                .any(|local| local.nullable != locals[0].nullable)
            {
                report.error(&table.id, foreign.name.as_deref(), CatalogCode::UnsupportedAttribute, "Composite foreign-key columns with mixed nullability cannot share one BSL relation");
                continue;
            }
            if table
                .primary_key
                .as_ref()
                .is_some_and(|key| locals.iter().any(|local| key.columns.contains(&local.name)))
            {
                report.error(
                    &table.id,
                    foreign.name.as_deref(),
                    CatalogCode::UnsupportedAttribute,
                    "A composite foreign key inside the primary key requires manual representation",
                );
                continue;
            }
        }
        if locals
            .iter()
            .any(|local| !occupied.insert(local.name.clone()))
        {
            report.error(
                &table.id,
                foreign.name.as_deref(),
                CatalogCode::UnsupportedAttribute,
                "Overlapping declared foreign keys cannot occupy one BSL field",
            );
            continue;
        }
        let first = table
            .columns
            .iter()
            .find(|column| foreign.columns.contains(&column.name))
            .map_or_else(|| foreign.columns[0].clone(), |column| column.name.clone());
        relations.insert(first, foreign);
        if foreign.name.is_some() {
            report.loss(&table.id, foreign.name.as_deref(), "BSL preserves FK pairs/actions but generates its own constraint name for future DDL");
        }
    }
    relations
}

/// The local columns of `foreign`, when every pair keeps its native type and carries no
/// default or generation the relation would drop.
fn representable_pairs<'a>(
    catalog: &Catalog,
    table: &'a Table,
    target: &Table,
    foreign: &ForeignKey,
    report: &mut ImportReport,
) -> Option<Vec<&'a Column>> {
    let mut locals = Vec::new();
    for (local, remote) in foreign.columns.iter().zip(&foreign.target_columns) {
        let local = table.columns.iter().find(|column| column.name == *local)?;
        let remote = target
            .columns
            .iter()
            .find(|column| column.name == *remote)?;
        if !compatible(local, remote, &table.id, catalog, report)
            || local.default.is_some()
            || local.generation != ValueGeneration::None
        {
            report.error(&table.id, Some(&local.name), CatalogCode::UnsupportedAttribute, "Relation would lose native type/default/generation semantics; retain it for manual representation");
            return None;
        }
        locals.push(local);
    }
    Some(locals)
}

/// The `(column …)` body: the local column name, or one `part = "column"` per target
/// key part of a composite FK.
pub(crate) fn columns(foreign: &ForeignKey, target: &ModelNames) -> String {
    if let [local] = foreign.columns.as_slice() {
        return quote(local);
    }
    foreign
        .columns
        .iter()
        .zip(&foreign.target_columns)
        .map(|(local, remote)| format!("{} = {}", target.fields[remote], quote(local)))
        .collect::<Vec<_>>()
        .join(", ")
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
