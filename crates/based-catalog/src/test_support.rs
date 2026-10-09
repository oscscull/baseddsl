//! Small shared assertions for independently authored dialect-reader fixtures.
use super::{Catalog, Discovery, Selection};

pub fn assert_canonical(discovery: &Discovery) {
    assert_eq!(discovery.catalog, discovery.catalog.clone().canonicalize());
    let mut findings = discovery.diagnostics.clone();
    findings.sort();
    findings.dedup();
    assert_eq!(findings, discovery.diagnostics);
}

pub fn assert_selected(catalog: &Catalog, selection: &Selection) {
    assert_eq!(
        catalog
            .tables
            .iter()
            .map(|table| &table.id)
            .collect::<Vec<_>>(),
        selection.tables().collect::<Vec<_>>()
    );
}

pub fn assert_supported(discovery: &Discovery, selection: &Selection) {
    assert_canonical(discovery);
    assert_selected(&discovery.catalog, selection);
    assert!(!discovery.has_errors(), "{:?}", discovery.diagnostics);
}
