#[path = "support/fixture.rs"]
mod fixture;
use based_catalog::*;
use based_catalog_bsl::{emit, ImportReport};

fn rejected(catalog: Catalog) -> ImportReport {
    let dialect = catalog.source.dialect;
    let (discovery, selection) = fixture::discovery(catalog);
    match emit(&discovery, &selection, &fixture::manifest(dialect, "none")) {
        Err(report) => {
            assert!(report.has_errors());
            report
        }
        Ok(_) => panic!("lossy catalog returned publishable files"),
    }
}

fn finding(report: &ImportReport, member: &str, code: CatalogCode) {
    assert!(
        report
            .diagnostics
            .iter()
            .any(|finding| finding.member.as_deref() == Some(member) && finding.code == code),
        "{report:?}"
    );
}

#[test]
fn generation_and_temporal_numeric_variants_are_rejected_without_approximation() {
    for (dialect, generation) in [
        (
            CatalogDialect::Sqlite,
            ValueGeneration::SqliteRowId {
                autoincrement: false,
            },
        ),
        (
            CatalogDialect::Postgres,
            ValueGeneration::Identity { always: false },
        ),
        (
            CatalogDialect::Postgres,
            ValueGeneration::Sequence {
                expression: "nextval('old_seq'::regclass)".into(),
            },
        ),
    ] {
        let mut catalog = fixture::catalog(dialect);
        catalog.tables[0].columns[0].generation = generation;
        finding(
            &rejected(catalog),
            "odd id",
            CatalogCode::UnsupportedAttribute,
        );
    }
    for (dialect, native) in [
        (CatalogDialect::Postgres, {
            let mut native = NativeType::declared("numeric(5,-2)", TypeFamily::Decimal);
            native.precision = Some(5);
            native.scale = Some(-2);
            native
        }),
        (CatalogDialect::Postgres, {
            let mut native =
                NativeType::declared("timestamp without time zone", TypeFamily::Timestamp);
            native.timezone = Some(false);
            native
        }),
        (
            CatalogDialect::MariaDb,
            NativeType::declared("time", TypeFamily::Time),
        ),
        (
            CatalogDialect::MariaDb,
            NativeType::declared("uuid", TypeFamily::Uuid),
        ),
    ] {
        let mut catalog = fixture::catalog(dialect);
        catalog.tables[0].columns[1].native_type = native;
        finding(&rejected(catalog), "label", CatalogCode::UnsupportedType);
    }
}

#[test]
fn unknown_defaults_and_unrepresentable_aliases_are_blocking_reports() {
    for source in ["-5", "+5", "-1.25"] {
        let mut catalog = fixture::catalog(CatalogDialect::Postgres);
        catalog.tables[0].columns[1].native_type =
            NativeType::declared("bigint", TypeFamily::Integer);
        catalog.tables[0].columns[1].default = Some(source.into());
        finding(
            &rejected(catalog),
            "label",
            CatalogCode::UnsupportedAttribute,
        );
    }
    for source in [
        "random()",
        "CURRENT_TIMESTAMP",
        "'value'::unknown_domain",
        "'broken'quote'",
    ] {
        let mut catalog = fixture::catalog(CatalogDialect::Postgres);
        catalog.tables[0].columns[1].default = Some(source.into());
        finding(
            &rejected(catalog),
            "label",
            CatalogCode::UnsupportedAttribute,
        );
    }
    let mut catalog = fixture::catalog(CatalogDialect::MariaDb);
    catalog.tables[0].columns[1].default = Some("'mode\\dependent'".into());
    finding(
        &rejected(catalog),
        "label",
        CatalogCode::UnsupportedAttribute,
    );
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    catalog.tables[4].id.name = "dotted.table".into();
    assert!(rejected(catalog)
        .diagnostics
        .iter()
        .any(|finding| finding.message.contains("alias cannot")));
    let mut catalog = fixture::catalog(CatalogDialect::Postgres);
    catalog.tables[4].id.namespace = "schema with spaces".into();
    assert!(rejected(catalog)
        .diagnostics
        .iter()
        .any(|finding| finding.message.contains("alias cannot")));
}

#[test]
fn composite_or_alternate_key_foreign_mappings_do_not_become_guessed_edges() {
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    let child = &mut catalog.tables[1];
    child.foreign_keys = vec![ForeignKey {
        name: Some("native_composite".into()),
        columns: vec!["part z".into(), "part a".into()],
        target: child.id.clone(),
        target_columns: vec!["part z".into(), "part a".into()],
        on_delete: ReferentialAction::NoAction,
        on_update: ReferentialAction::NoAction,
        match_mode: MatchMode::Simple,
        deferral: Deferral::NotDeferrable,
    }];
    finding(
        &rejected(catalog),
        "native_composite",
        CatalogCode::UnsupportedAttribute,
    );
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    catalog.tables[1].foreign_keys[0].target_columns = vec!["label".into()];
    catalog.tables[1].foreign_keys[0].name = Some("alternate_unique".into());
    catalog.tables[0].unique_keys.push(Key {
        name: None,
        columns: vec!["label".into()],
        deferral: Deferral::NotDeferrable,
    });
    finding(
        &rejected(catalog),
        "alternate_unique",
        CatalogCode::UnsupportedAttribute,
    );
}

#[test]
fn opaque_types_special_indexes_and_partial_selection_remain_failures() {
    let mut catalog = fixture::catalog(CatalogDialect::Postgres);
    catalog.tables[4].columns[0].native_type =
        NativeType::declared("custom_domain", TypeFamily::Domain);
    finding(&rejected(catalog), "id", CatalogCode::UnsupportedType);
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    catalog.tables[1].indexes[0].parts[0].collation = Some("NOCASE".into());
    finding(
        &rejected(catalog),
        "existing_parent_lookup",
        CatalogCode::UnsupportedAttribute,
    );
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    catalog.tables[1].indexes[0].predicate = Some("parent_ref IS NOT NULL".into());
    finding(
        &rejected(catalog),
        "existing_parent_lookup",
        CatalogCode::UnsupportedAttribute,
    );
    let catalog = fixture::catalog(CatalogDialect::Sqlite);
    let selection = Selection::new([catalog.tables[1].id.clone()]).unwrap();
    let mut discovery = Discovery::checked(
        Catalog {
            source: catalog.source,
            tables: vec![catalog.tables[1].clone()],
        },
        &selection,
        Vec::new(),
    );
    discovery.diagnostics.clear(); // Even a caller dropping findings cannot bypass revalidation.
    let Err(report) = emit(
        &discovery,
        &selection,
        &fixture::manifest(CatalogDialect::Sqlite, "none"),
    ) else {
        panic!("outside reference accepted")
    };
    assert!(report
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::OutsideSelection));
}

#[test]
fn nullable_native_key_is_a_compiler_error_and_cannot_be_published() {
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    catalog.tables[2].columns[0].nullable = true;
    let report = rejected(catalog);
    assert!(report
        .compiler
        .as_ref()
        .is_some_and(|report| report.errors() > 0));
}
