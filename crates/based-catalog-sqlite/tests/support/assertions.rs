use based_catalog::{Discovery, SqliteAffinity, ValueGeneration};

pub fn physical_facts(discovery: &Discovery) {
    let table = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "odd\" table")
        .unwrap();
    assert!(table.without_rowid);
    assert_eq!(
        table.primary_key.as_ref().unwrap().columns,
        ["part z", "part a"]
    );
    assert_eq!(table.unique_keys[0].columns, ["part a", "part z"]);
    assert_eq!(table.foreign_keys[0].columns, ["next_z", "next_a"]);
    assert_eq!(table.foreign_keys[0].target_columns, ["part z", "part a"]);
    assert_eq!(
        table.columns[2].native_type.sqlite_affinity,
        Some(SqliteAffinity::Text)
    );
    assert_eq!(
        table.columns[2].default.as_deref(),
        Some("'it''s retained'")
    );
    assert_eq!(table.columns[3].default.as_deref(), Some("10.125"));
    let keyless = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "keyless")
        .unwrap();
    assert!(keyless.primary_key.is_none());
    assert!(keyless.foreign_keys.is_empty());
    for (name, autoincrement) in [("rowid_auto", true), ("rowid_reuse", false)] {
        let table = discovery
            .catalog
            .tables
            .iter()
            .find(|table| table.id.name == name)
            .unwrap();
        assert_eq!(
            table.columns[0].generation,
            ValueGeneration::SqliteRowId { autoincrement }
        );
        assert!(!table.columns[0].nullable);
    }
    let strict = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "strict_values")
        .unwrap();
    assert!(strict.strict);
    assert!(!strict.columns[0].nullable);
    assert_eq!(strict.columns[0].generation, ValueGeneration::None);
    let mut arrival = discovery.catalog.clone();
    arrival.tables.reverse();
    for table in &mut arrival.tables {
        table.columns.reverse();
        table.indexes.reverse();
    }
    assert_eq!(arrival.canonicalize(), discovery.catalog);
}
