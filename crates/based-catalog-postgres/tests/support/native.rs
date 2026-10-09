use based_catalog::{CatalogCode, Discovery, QualifiedName, TypeFamily, ValueGeneration};

pub fn native_facts(discovery: &Discovery) {
    based_catalog::test_support::assert_canonical(discovery);
    for code in [
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
    let columns = &table.columns;
    assert_eq!(
        columns[0].generation,
        ValueGeneration::Identity { always: true }
    );
    assert!(matches!(
        columns[1].generation,
        ValueGeneration::Sequence { .. }
    ));
    assert_eq!(
        columns[2].native_type.named_type,
        Some(QualifiedName::new("catalog fixture", "mood"))
    );
    assert_eq!(columns[2].native_type.enum_values, ["second", "first"]);
    assert_eq!(columns[3].native_type.family, TypeFamily::Domain);
    assert_eq!(
        columns[3].native_type.named_type,
        Some(QualifiedName::new("catalog fixture", "positive_number"))
    );
    assert!(!columns[3].nullable);
    assert_eq!(columns[4].native_type.family, TypeFamily::Array);
    assert!(matches!(
        columns[5].generation,
        ValueGeneration::Generated { stored: true, .. }
    ));
    assert_eq!(columns[7].native_type.precision, Some(5));
    assert_eq!(columns[7].native_type.scale, Some(-2));
    assert_eq!(columns[8].native_type.timezone, Some(true));
    assert_eq!(columns[8].native_type.precision, Some(3));
    assert_eq!(table.checks.len(), 1);
    let index = table
        .indexes
        .iter()
        .find(|index| index.name == "expression_partial")
        .unwrap();
    assert!(index.predicate.is_some());
    assert_eq!(index.included_columns, ["sequence_id"]);
    assert!(index
        .native_definition
        .as_ref()
        .unwrap()
        .contains("lower(label)"));
}

pub fn foreign_semantics(discovery: &Discovery) {
    let table = discovery
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "foreign_semantics")
        .unwrap();
    assert_eq!(
        table.primary_key.as_ref().unwrap().deferral,
        based_catalog::Deferral::InitiallyDeferred
    );
    let foreign_key = &table.foreign_keys[0];
    assert_eq!(foreign_key.match_mode, based_catalog::MatchMode::Full);
    assert_eq!(
        foreign_key.on_delete,
        based_catalog::ReferentialAction::SetDefault
    );
    assert_eq!(
        foreign_key.deferral,
        based_catalog::Deferral::InitiallyDeferred
    );
    assert!(discovery
        .diagnostics
        .iter()
        .any(|finding| finding.member.as_deref() == Some("unvalidated")));
}
