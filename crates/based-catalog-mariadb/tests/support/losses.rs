use based_catalog::{CatalogCode, Discovery};

pub fn losses(discovery: &Discovery) {
    based_catalog::test_support::assert_canonical(discovery);
    assert!(discovery.has_errors());
    for code in [
        CatalogCode::UnsupportedType,
        CatalogCode::UnsupportedAttribute,
        CatalogCode::UnsupportedObject,
        CatalogCode::IncompleteMetadata,
    ] {
        assert!(
            discovery
                .diagnostics
                .iter()
                .any(|finding| finding.code == code),
            "missing {code:?}"
        );
    }
    let table = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "native_details")
        .unwrap();
    assert_eq!(table.checks.len(), 1);
    assert!(table.checks[0].expression.contains("unsigned_value"));
    let definition = table.native_definition.as_ref().unwrap();
    for preserved in ["enum", "INVISIBLE", "ON UPDATE", "GENERATED ALWAYS"] {
        assert!(definition.contains(preserved), "missing {preserved}");
    }
    assert_eq!(
        table
            .indexes
            .iter()
            .find(|index| index.name == "text_prefix")
            .unwrap()
            .parts[0]
            .prefix_length,
        Some(3)
    );
}
