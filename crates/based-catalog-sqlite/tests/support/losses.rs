use based_catalog::{
    CatalogCode, Deferral, Discovery, IndexTarget, SqliteAffinity, ValueGeneration,
};

pub fn limitations(discovery: &Discovery) {
    based_catalog::test_support::assert_canonical(discovery);
    assert!(discovery.has_errors());
    for code in [
        CatalogCode::IncompleteMetadata,
        CatalogCode::UnsupportedType,
        CatalogCode::UnsupportedAttribute,
        CatalogCode::UnsupportedObject,
    ] {
        assert!(discovery
            .diagnostics
            .iter()
            .any(|finding| finding.code == code));
    }
    let table = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "native_details")
        .unwrap();
    assert_eq!(
        table.columns[2].native_type.sqlite_affinity,
        Some(SqliteAffinity::Numeric)
    );
    assert!(matches!(
        table.columns[4].generation,
        ValueGeneration::Generated {
            stored: false,
            expression: None
        }
    ));
    assert!(matches!(
        table.columns[5].generation,
        ValueGeneration::Generated {
            stored: true,
            expression: None
        }
    ));
    let index = &table.indexes[0];
    assert!(matches!(
        index.parts[0].target,
        IndexTarget::Expression(None)
    ));
    assert!(index
        .native_definition
        .as_ref()
        .unwrap()
        .contains("WHERE label IS NOT NULL"));
    let foreign = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "foreign_semantics")
        .unwrap();
    assert_eq!(foreign.foreign_keys[0].deferral, Deferral::Unknown);
    assert_eq!(foreign.foreign_keys[0].target_columns, ["id"]);
    let descending = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "descending_pk")
        .unwrap();
    assert!(descending.columns[0].nullable);
    assert_eq!(descending.columns[0].generation, ValueGeneration::None);
    let untyped = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "untyped")
        .unwrap();
    assert!(untyped.columns[0].native_type.declaration.is_empty());
    assert_eq!(
        untyped.columns[0].native_type.sqlite_affinity,
        Some(SqliteAffinity::Blob)
    );
}
