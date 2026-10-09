//! Adversarial contracts that all independent metadata readers must share.
mod support;
use based_catalog::*;
use support::*;

#[test]
fn order_is_canonical_but_composite_parts_and_cycles_remain_intact() {
    let mut a = table("A unusual");
    let mut b = table("a_unusual");
    a.foreign_keys.push(relation("a_unusual"));
    b.foreign_keys.push(relation("A unusual"));
    let selected = selection(&["a_unusual", "A unusual"]);
    let first = Discovery::checked(catalog(vec![b.clone(), a.clone()]), &selected, Vec::new());
    a.columns.reverse();
    b.columns.reverse();
    let second = Discovery::checked(catalog(vec![a, b]), &selected, Vec::new());
    assert_eq!(first, second);
    assert!(!first.has_errors());
    assert_eq!(
        first.catalog.tables[0]
            .primary_key
            .as_ref()
            .unwrap()
            .columns,
        ["owner", "seq"]
    );
    assert_eq!(
        first.catalog.tables[0].foreign_keys[0].target_columns,
        ["owner", "seq"]
    );
    #[cfg(feature = "test-support")]
    test_support::assert_supported(&first, &selected);
}

#[test]
fn selection_is_explicit_and_never_silently_expanded() {
    assert!(Selection::new([]).is_err());
    assert!(Selection::new([TableId::new("", "table")]).is_err());
    let mut a = table("a");
    a.foreign_keys.push(relation("b"));
    let selected = selection(&["a"]);
    let discovery = Discovery::checked(catalog(vec![a]), &selected, Vec::new());
    assert!(discovery.has_errors());
    assert_eq!(discovery.catalog.tables.len(), 1);
    assert!(discovery
        .diagnostics
        .iter()
        .any(|issue| issue.code == CatalogCode::OutsideSelection));
    assert_eq!(
        discovery.catalog.tables[0].foreign_keys[0].target,
        TableId::new("public", "b")
    );
    let expanded = Discovery::checked(catalog(vec![table("a"), table("b")]), &selected, Vec::new());
    assert!(expanded
        .diagnostics
        .iter()
        .any(|issue| issue.code == CatalogCode::UnexpectedTable));
}

#[test]
fn missing_duplicate_and_dangling_metadata_are_blocking() {
    let mut a = table("a");
    a.columns.push(column("seq", 3));
    a.primary_key = Some(key(&["missing"]));
    let discovery = Discovery::checked(
        catalog(vec![a.clone(), a]),
        &selection(&["a", "missing"]),
        Vec::new(),
    );
    for code in [
        CatalogCode::DuplicateFact,
        CatalogCode::InvalidReference,
        CatalogCode::MissingTable,
    ] {
        assert!(discovery.diagnostics.iter().any(|issue| issue.code == code));
    }
    assert!(discovery.has_errors());
}

#[test]
fn native_details_and_adapter_losses_survive_serialization() {
    let mut a = table("a");
    let native = &mut a.columns[0].native_type;
    native.declaration = "numeric(12,2)".into();
    native.family = TypeFamily::Decimal;
    native.precision = Some(12);
    native.scale = Some(2);
    a.columns[0].default = Some("(1.25::numeric)".into());
    let loss = CatalogDiagnostic {
        severity: CatalogSeverity::Error,
        code: CatalogCode::UnsupportedAttribute,
        table: a.id.clone(),
        member: Some("seq".into()),
        message: "Default expression needs review".into(),
    };
    let discovery = Discovery::checked(
        catalog(vec![a]),
        &selection(&["a"]),
        vec![loss.clone(), loss],
    );
    let bytes = serde_json::to_vec(&discovery).unwrap();
    let restored: Discovery = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(discovery, restored);
    assert_eq!(discovery.diagnostics.len(), 1);
    assert!(discovery.has_errors());
}

#[test]
fn operational_errors_have_no_url_or_driver_payload() {
    for error in [
        CatalogReadError::Connection,
        CatalogReadError::Metadata,
        CatalogReadError::InconsistentSnapshot,
    ] {
        assert!(!error.to_string().contains("://"));
        assert!(!format!("{error:?}").contains("password"));
        assert!(!error.to_string().is_empty());
    }
}
