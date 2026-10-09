use based_catalog::Discovery;

pub fn physical_facts(discovery: &Discovery) {
    let composite = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "odd\" table")
        .unwrap();
    assert_eq!(
        composite.primary_key.as_ref().unwrap().columns,
        ["part z", "part a"]
    );
    assert_eq!(composite.unique_keys[0].columns, ["part a", "part z"]);
    assert_eq!(composite.foreign_keys[0].columns, ["next_z", "next_a"]);
    assert_eq!(
        composite.foreign_keys[0].target_columns,
        ["part z", "part a"]
    );
    assert_eq!(composite.columns[2].native_type.length, Some(80));
    assert_eq!(
        composite.columns[2].collation.as_deref(),
        Some("pg_catalog.\"C\"")
    );
    assert!(composite.columns[2].nullable);
    assert!(composite.columns[2]
        .default
        .as_ref()
        .unwrap()
        .contains("retained"));
    assert_eq!(composite.columns[3].native_type.precision, Some(12));
    assert_eq!(composite.columns[3].native_type.scale, Some(3));
    let keyless = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "keyless")
        .unwrap();
    assert!(keyless.primary_key.is_none());
    assert!(keyless.foreign_keys.is_empty());
    for name in ["cycle_a", "cycle_b"] {
        assert_eq!(
            discovery
                .catalog
                .tables
                .iter()
                .find(|table| table.id.name == name)
                .unwrap()
                .foreign_keys
                .len(),
            1
        );
    }
    let mut arrival = discovery.catalog.clone();
    arrival.tables.reverse();
    for table in &mut arrival.tables {
        table.columns.reverse();
        table.indexes.reverse();
        table.unique_keys.reverse();
    }
    assert_eq!(arrival.canonicalize(), discovery.catalog);
}
