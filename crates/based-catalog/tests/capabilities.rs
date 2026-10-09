//! Unsupported objects stay inspectable and cannot silently become supported imports.
mod support;
use based_catalog::*;
use support::*;

#[test]
fn native_opaque_semantics_are_blocking_and_retained() {
    for family in [
        TypeFamily::Other,
        TypeFamily::Enum,
        TypeFamily::Domain,
        TypeFamily::Array,
    ] {
        let mut a = table("a");
        a.columns[0].native_type = NativeType::declared("native_opaque_type", family);
        let result = Discovery::checked(catalog(vec![a]), &selection(&["a"]), Vec::new());
        assert!(result.has_errors());
        assert_eq!(
            result.catalog.tables[0].columns[1].native_type.declaration,
            "native_opaque_type"
        );
        assert!(result
            .diagnostics
            .iter()
            .any(|issue| issue.code == CatalogCode::UnsupportedType));
    }
}

#[test]
fn generated_unsigned_and_view_facts_are_not_dropped() {
    let mut a = table("a");
    a.kind = TableKind::View;
    a.columns[0].generation = ValueGeneration::Generated {
        expression: Some("owner + 1".into()),
        stored: true,
    };
    a.columns[0].native_type.unsigned = true;
    let result = Discovery::checked(catalog(vec![a]), &selection(&["a"]), Vec::new());
    assert!(result.has_errors());
    assert_eq!(result.catalog.tables[0].kind, TableKind::View);
    assert!(matches!(
        result.catalog.tables[0].columns[1].generation,
        ValueGeneration::Generated { .. }
    ));
    assert_eq!(result.diagnostics.len(), 3);
}

#[test]
fn partial_expression_index_is_inspectable_but_not_success() {
    let mut a = table("a");
    a.indexes.push(Index {
        name: "special_index".into(),
        origin: IndexOrigin::Explicit,
        unique: false,
        parts: vec![IndexPart {
            target: IndexTarget::Expression(Some("owner + seq".into())),
            direction: IndexDirection::Ascending,
            prefix_length: None,
            collation: None,
        }],
        included_columns: Vec::new(),
        method: None,
        predicate: Some("seq > 0".into()),
        native_definition: Some(
            "CREATE INDEX special_index ON a ((owner + seq)) WHERE seq > 0".into(),
        ),
        valid: true,
    });
    let result = Discovery::checked(catalog(vec![a]), &selection(&["a"]), Vec::new());
    assert!(result.has_errors());
    assert_eq!(
        result.catalog.tables[0].indexes[0].predicate.as_deref(),
        Some("seq > 0")
    );
}

#[test]
fn keyless_does_not_invent_a_primary_key() {
    let mut a = table("a");
    a.primary_key = None;
    let result = Discovery::checked(catalog(vec![a]), &selection(&["a"]), Vec::new());
    assert!(!result.has_errors());
    assert!(result.catalog.tables[0].primary_key.is_none());
}

#[test]
fn nonstandard_declared_fk_semantics_are_retained_and_blocking() {
    let mut a = table("a");
    let mut foreign = relation("a");
    foreign.deferral = Deferral::InitiallyDeferred;
    foreign.match_mode = MatchMode::Full;
    foreign.on_delete = ReferentialAction::SetDefault;
    a.foreign_keys.push(foreign);
    let result = Discovery::checked(catalog(vec![a]), &selection(&["a"]), Vec::new());
    assert!(result.has_errors());
    assert_eq!(
        result.catalog.tables[0].foreign_keys[0].match_mode,
        MatchMode::Full
    );
}

#[test]
fn unavailable_foreign_key_timing_cannot_pass_as_immediate() {
    let mut a = table("a");
    let mut foreign = relation("a");
    foreign.deferral = Deferral::Unknown;
    a.foreign_keys.push(foreign);
    let result = Discovery::checked(catalog(vec![a]), &selection(&["a"]), Vec::new());
    assert!(result.has_errors());
    assert_eq!(
        result.catalog.tables[0].foreign_keys[0].deferral,
        Deferral::Unknown
    );
    assert!(result
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::UnsupportedAttribute));
}

#[test]
fn sqlite_untyped_declaration_is_a_known_unsupported_fact() {
    let mut a = table("a");
    let mut native = NativeType::declared("", TypeFamily::Other);
    native.sqlite_affinity = Some(SqliteAffinity::Blob);
    a.columns[0].native_type = native;
    let mut source = catalog(vec![a]);
    source.source.dialect = CatalogDialect::Sqlite;
    let result = Discovery::checked(source, &selection(&["a"]), Vec::new());
    assert!(result
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::UnsupportedType));
    assert!(!result
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::IncompleteMetadata));
}
