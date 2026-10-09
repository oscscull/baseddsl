//! The conservative automatic-import profile, separate from physical catalog facts.
mod columns;
mod foreign_keys;
mod indexes;
mod tables;
use crate::{Catalog, CatalogDiagnostic};

pub fn assess(catalog: &Catalog) -> Vec<CatalogDiagnostic> {
    catalog
        .tables
        .iter()
        .flat_map(|table| {
            tables::assess(table)
                .into_iter()
                .chain(columns::assess(table))
                .chain(indexes::assess(table))
                .chain(foreign_keys::assess(table))
        })
        .collect()
}
