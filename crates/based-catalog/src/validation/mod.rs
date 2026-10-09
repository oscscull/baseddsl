//! Validate catalog structure and selected coverage, without interpreting SQL.
mod columns;
mod foreign_keys;
mod indexes;
mod keys;
use crate::{Catalog, CatalogCode, CatalogDiagnostic, CatalogSeverity, Selection, Table, TableId};
use std::collections::BTreeSet;
pub(crate) fn finding(
    table: &TableId,
    member: Option<&str>,
    code: CatalogCode,
    message: &str,
) -> CatalogDiagnostic {
    CatalogDiagnostic {
        severity: CatalogSeverity::Error,
        code,
        table: table.clone(),
        member: member.map(str::to_owned),
        message: message.to_owned(),
    }
}

pub fn validate(catalog: &Catalog, selection: &Selection) -> Vec<CatalogDiagnostic> {
    let mut findings = Vec::new();
    let mut seen = BTreeSet::new();
    for table in &catalog.tables {
        if !seen.insert(&table.id) {
            findings.push(finding(
                &table.id,
                None,
                CatalogCode::DuplicateFact,
                "Duplicate physical table identity",
            ));
        }
        if !selection.contains(&table.id) {
            findings.push(finding(
                &table.id,
                None,
                CatalogCode::UnexpectedTable,
                "Discovery expanded the explicit table selection",
            ));
        }
        findings.extend(columns::validate(table));
        findings.extend(keys::validate(table));
        findings.extend(indexes::validate(table));
        findings.extend(foreign_keys::validate(table, catalog, selection));
    }
    findings.extend(
        selection
            .tables()
            .filter(|id| !seen.contains(id))
            .map(|id| {
                finding(
                    id,
                    None,
                    CatalogCode::MissingTable,
                    "Selected table is missing or metadata is not visible",
                )
            }),
    );
    findings
}

fn has_column(table: &Table, name: &str) -> bool {
    table.columns.iter().any(|column| column.name == name)
}
