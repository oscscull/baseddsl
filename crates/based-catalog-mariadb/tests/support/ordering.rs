use based_catalog::{Discovery, Selection};

pub fn assert_arrival_independent(discovery: &Discovery, selection: &Selection) {
    let mut catalog = discovery.catalog.clone();
    catalog.tables.reverse();
    for table in &mut catalog.tables {
        table.columns.reverse();
        table.unique_keys.reverse();
        table.indexes.reverse();
        table.foreign_keys.reverse();
        table.checks.reverse();
    }
    assert_eq!(
        *discovery,
        Discovery::checked(catalog, selection, Vec::new())
    );
}
