use based_catalog::{Discovery, TypeFamily};

pub fn physical_facts(discovery: &Discovery) {
    let tables = &discovery.catalog.tables;
    let composite = tables
        .iter()
        .find(|table| table.id.name == "odd` table")
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
        Some("utf8mb4_bin")
    );
    assert!(composite.columns[2].nullable);
    assert!(composite.columns[2]
        .default
        .as_ref()
        .unwrap()
        .contains("retained"));
    assert_eq!(composite.columns[3].native_type.family, TypeFamily::Decimal);
    assert_eq!(composite.columns[3].native_type.precision, Some(12));
    assert_eq!(composite.columns[3].native_type.scale, Some(3));
    assert_eq!(composite.columns[3].default.as_deref(), Some("10.125"));
    let keyless = tables
        .iter()
        .find(|table| table.id.name == "keyless")
        .unwrap();
    assert!(keyless.primary_key.is_none());
    assert!(keyless.foreign_keys.is_empty());
    for name in ["cycle_a", "cycle_b"] {
        assert_eq!(
            tables
                .iter()
                .find(|table| table.id.name == name)
                .unwrap()
                .foreign_keys
                .len(),
            1
        );
    }
}
