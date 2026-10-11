#[path = "support/fixture.rs"]
mod fixture;
use based_catalog::*;
use based_catalog_bsl::{emit, Emission, ImportReport};
use based_sema::MemberKind;

/// `orders(tenant_id, "order no")` keyed together; `lines` points at it through unrelated,
/// quoted local names, plus an index over the FK pair.
fn catalog(dialect: CatalogDialect, nullable: bool) -> Catalog {
    let mut catalog = fixture::catalog(dialect);
    let namespace = catalog.tables[0].id.namespace.clone();
    let integer = if dialect == CatalogDialect::Sqlite {
        "INTEGER"
    } else {
        "bigint"
    };
    let orders = fixture::table(
        &namespace,
        "orders",
        vec![
            fixture::column("tenant_id", 0, integer, TypeFamily::Integer, false),
            fixture::column("order no", 1, integer, TypeFamily::Integer, false),
        ],
        &["tenant_id", "order no"],
    );
    let mut lines = fixture::table(
        &namespace,
        "lines",
        vec![
            fixture::column("line_id", 0, integer, TypeFamily::Integer, false),
            fixture::column("ord no", 1, integer, TypeFamily::Integer, nullable),
            fixture::column("orders", 2, integer, TypeFamily::Integer, false),
            fixture::column("ord\"tenant", 3, integer, TypeFamily::Integer, nullable),
        ],
        &["line_id"],
    );
    lines.foreign_keys.push(composite(
        &["ord\"tenant", "ord no"],
        &orders.id,
        &["tenant_id", "order no"],
    ));
    lines.indexes.push(Index {
        name: "lines_order".into(),
        origin: IndexOrigin::Explicit,
        unique: false,
        parts: ["ord\"tenant", "ord no"]
            .into_iter()
            .map(|column| IndexPart {
                target: IndexTarget::Column(column.into()),
                direction: IndexDirection::Ascending,
                prefix_length: None,
                collation: None,
            })
            .collect(),
        included_columns: Vec::new(),
        method: None,
        predicate: None,
        native_definition: None,
        valid: true,
    });
    catalog.tables = vec![orders, lines];
    catalog
}

fn composite(local: &[&str], target: &TableId, remote: &[&str]) -> ForeignKey {
    ForeignKey {
        name: None,
        columns: local.iter().map(|column| (*column).into()).collect(),
        target: target.clone(),
        target_columns: remote.iter().map(|column| (*column).into()).collect(),
        on_delete: ReferentialAction::Cascade,
        on_update: ReferentialAction::NoAction,
        match_mode: MatchMode::Simple,
        deferral: Deferral::NotDeferrable,
    }
}

fn emitted(catalog: Catalog) -> Emission {
    let dialect = catalog.source.dialect;
    let (discovery, selection) = fixture::discovery(catalog);
    emit(&discovery, &selection, &fixture::manifest(dialect, "all"))
        .unwrap_or_else(|report| panic!("{report:?}"))
}

fn rejected(catalog: Catalog) -> ImportReport {
    let dialect = catalog.source.dialect;
    let (discovery, selection) = fixture::discovery(catalog);
    match emit(&discovery, &selection, &fixture::manifest(dialect, "all")) {
        Err(report) => report,
        Ok(_) => panic!("unrepresentable composite FK returned publishable files"),
    }
}

/// The compiler-resolved `(local, remote)` pairs of every forward field, in key order.
fn resolved_pairs(output: &Emission, model: &str) -> Vec<Vec<(String, String)>> {
    let schema = &output.checked.schema;
    schema
        .model(model)
        .unwrap()
        .members
        .iter()
        .filter(|member| matches!(member.kind, MemberKind::Forward { .. }))
        .map(|member| {
            schema
                .fk_columns(member)
                .into_iter()
                .map(|(column, part)| (column, part.physical_col().to_owned()))
                .collect()
        })
        .collect()
}

fn catalog_pairs(foreign: &ForeignKey) -> Vec<(String, String)> {
    foreign
        .columns
        .iter()
        .cloned()
        .zip(foreign.target_columns.iter().cloned())
        .collect()
}

#[test]
fn composite_fk_aliases_round_trip_on_every_dialect() {
    for dialect in [
        CatalogDialect::Sqlite,
        CatalogDialect::Postgres,
        CatalogDialect::MariaDb,
    ] {
        for nullable in [true, false] {
            let catalog = catalog(dialect, nullable);
            let expected = catalog_pairs(&catalog.tables[1].foreign_keys[0]);
            let output = emitted(catalog);
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert_eq!(resolved_pairs(&output, "Lines"), [expected]);
            // The column named `orders` keeps its field; the relation takes the next name.
            let lines = output.checked.schema.model("Lines").unwrap();
            let MemberKind::Forward { optional, .. } = lines.member("orders_2").unwrap().kind
            else {
                panic!()
            };
            assert_eq!(optional, nullable);
            assert!(matches!(
                lines.member("orders").unwrap().kind,
                MemberKind::Scalar { .. }
            ));
            assert_eq!(lines.indexes[0].columns, ["orders_2"]);
            let source = &output
                .files
                .iter()
                .find(|(path, _)| path.ends_with("lines.bsl"))
                .unwrap()
                .1;
            assert!(
                source.contains(r#"(column tenant_id = "ord\"tenant", order_no = "ord no")"#),
                "{source}"
            );
        }
    }
}

#[test]
fn composite_fk_cycles_resolve_both_edges() {
    let mut catalog = catalog(CatalogDialect::Postgres, true);
    let lines_id = catalog.tables[1].id.clone();
    let orders = &mut catalog.tables[0];
    orders.columns.push(fixture::column(
        "first_line",
        2,
        "bigint",
        TypeFamily::Integer,
        true,
    ));
    orders
        .foreign_keys
        .push(fixture::foreign("first_line", &lines_id, "line_id"));
    let orders_id = orders.id.clone();
    let lines = &mut catalog.tables[1];
    lines.columns.push(fixture::column(
        "back_tenant",
        4,
        "bigint",
        TypeFamily::Integer,
        true,
    ));
    lines.columns.push(fixture::column(
        "back_no",
        5,
        "bigint",
        TypeFamily::Integer,
        true,
    ));
    lines.foreign_keys.push(composite(
        &["back_tenant", "back_no"],
        &orders_id,
        &["tenant_id", "order no"],
    ));
    let mut expected: Vec<_> = catalog.tables[1]
        .foreign_keys
        .iter()
        .map(catalog_pairs)
        .collect();
    let output = emitted(catalog);
    let mut actual = resolved_pairs(&output, "Lines");
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    assert_eq!(
        resolved_pairs(&output, "Orders"),
        [vec![("first_line".to_owned(), "line_id".to_owned())]]
    );
}

#[test]
fn unrepresentable_composite_fks_stay_blocking() {
    // Mixed nullability across the parts.
    let mut mixed = catalog(CatalogDialect::Sqlite, true);
    mixed.tables[1].columns[1].nullable = false;
    // Target key parts referenced out of key order.
    let mut reordered = catalog(CatalogDialect::Sqlite, true);
    let fk = &mut reordered.tables[1].foreign_keys[0];
    fk.columns.reverse();
    fk.target_columns.reverse();
    // Two composite FKs sharing a local column.
    let mut shared = catalog(CatalogDialect::Sqlite, true);
    let target = shared.tables[0].id.clone();
    shared.tables[1].columns.push(fixture::column(
        "other_no",
        4,
        "INTEGER",
        TypeFamily::Integer,
        true,
    ));
    shared.tables[1].foreign_keys.push(composite(
        &["ord\"tenant", "other_no"],
        &target,
        &["tenant_id", "order no"],
    ));
    for catalog in [mixed, reordered, shared] {
        assert!(rejected(catalog)
            .diagnostics
            .iter()
            .any(|finding| finding.code == CatalogCode::UnsupportedAttribute));
    }
    // An index over the FK pair in the opposite order is caught by verification.
    let mut index = catalog(CatalogDialect::Sqlite, true);
    index.tables[1].indexes[0].parts.reverse();
    assert!(rejected(index)
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::InvalidReference));
}
