#[path = "support/fixture.rs"]
mod fixture;
use based_ast::{DefaultVal, Literal, Primitive};
use based_catalog::*;
use based_catalog_bsl::emit;
use based_sema::MemberKind;

#[test]
fn native_scalars_literal_defaults_and_bound_losses_are_independent_compiler_facts() {
    let mut catalog = fixture::catalog(CatalogDialect::Postgres);
    let cases = cases();
    let columns = cases
        .iter()
        .enumerate()
        .map(|(position, (native, _, default, _))| {
            let mut column = fixture::column(
                &format!("value_{position}"),
                u32::try_from(position).unwrap(),
                "ignored",
                native.family,
                true,
            );
            column.native_type = native.clone();
            column.default = default.map(str::to_owned);
            column
        })
        .collect();
    catalog.tables = vec![fixture::table("app", "native_scalars", columns, &[])];
    let (discovery, selection) = fixture::discovery(catalog);
    let output = emit(
        &discovery,
        &selection,
        &fixture::manifest(CatalogDialect::Postgres, "none"),
    )
    .unwrap();
    let model = output.checked.schema.model("NativeScalars").unwrap();
    for (position, (_, expected_type, _, expected_default)) in cases.iter().enumerate() {
        let MemberKind::Scalar {
            ty,
            default,
            optional,
            ..
        } = &model.member(&format!("value_{position}")).unwrap().kind
        else {
            panic!()
        };
        assert_eq!(ty, expected_type);
        assert_eq!(default, expected_default);
        assert!(*optional);
    }
    assert!(output
        .report
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::DefinitionLoss
            && finding.member.as_deref() == Some("value_0")));
    assert!(!output.report.has_errors());
}

#[test]
fn signed_native_defaults_emit_as_signed_literals() {
    let int = |source: &str| (source.to_owned(), Literal::Int(-5));
    let fraction = |source: &str, text: &str| (source.to_owned(), Literal::Decimal(text.into()));
    for (dialect, declarations, cases) in [
        (
            CatalogDialect::Postgres,
            ["bigint", "numeric(6,2)", "double precision"],
            [
                int("'-5'::integer"),
                fraction("'-1.25'::numeric", "-1.25"),
                fraction("'-1.5'::double precision", "-1.5"),
            ],
        ),
        (
            CatalogDialect::MariaDb,
            ["bigint", "decimal(6,2)", "double"],
            [
                int("-5"),
                fraction("-1.25", "-1.25"),
                fraction("-1.5", "-1.5"),
            ],
        ),
        (
            CatalogDialect::Sqlite,
            ["integer", "real", "real"],
            [
                int("(-5)"),
                fraction("-1.25", "-1.25"),
                fraction("(-1.5)", "-1.5"),
            ],
        ),
    ] {
        let families = [TypeFamily::Integer, TypeFamily::Decimal, TypeFamily::Real];
        let columns = cases
            .iter()
            .enumerate()
            .map(|(position, (source, _))| {
                let mut family = families[position];
                if dialect == CatalogDialect::Sqlite && family == TypeFamily::Decimal {
                    family = TypeFamily::Real;
                }
                let mut column = fixture::column(
                    &format!("value_{position}"),
                    u32::try_from(position).unwrap(),
                    declarations[position],
                    family,
                    true,
                );
                if family == TypeFamily::Decimal {
                    column.native_type.precision = Some(6);
                    column.native_type.scale = Some(2);
                }
                column.default = Some(source.clone());
                column
            })
            .collect();
        let mut catalog = fixture::catalog(dialect);
        let namespace = catalog.tables[0].id.namespace.clone();
        catalog.tables = vec![fixture::table(&namespace, "signed", columns, &[])];
        let (discovery, selection) = fixture::discovery(catalog);
        let output = emit(&discovery, &selection, &fixture::manifest(dialect, "none")).unwrap();
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let model = output.checked.schema.model("Signed").unwrap();
        for (position, (_, expected)) in cases.iter().enumerate() {
            let MemberKind::Scalar { default, .. } =
                &model.member(&format!("value_{position}")).unwrap().kind
            else {
                panic!()
            };
            assert_eq!(
                default.as_ref(),
                Some(&DefaultVal::Lit(expected.clone())),
                "{dialect:?} {position}"
            );
        }
    }
}

type ScalarCase = (
    NativeType,
    Primitive,
    Option<&'static str>,
    Option<DefaultVal>,
);

fn cases() -> Vec<ScalarCase> {
    let mut time = NativeType::declared("time without time zone", TypeFamily::Time);
    time.timezone = Some(false);
    let mut timestamp = NativeType::declared("timestamp with time zone", TypeFamily::Timestamp);
    timestamp.timezone = Some(true);
    let mut decimal = NativeType::declared("numeric(16,3)", TypeFamily::Decimal);
    decimal.precision = Some(16);
    decimal.scale = Some(3);
    vec![
        (
            NativeType::declared("smallint", TypeFamily::Integer),
            Primitive::Int,
            Some("5"),
            Some(DefaultVal::Lit(Literal::Int(5))),
        ),
        (
            NativeType::declared("double precision", TypeFamily::Real),
            Primitive::Float,
            Some("1.25"),
            Some(DefaultVal::Lit(Literal::Decimal("1.25".into()))),
        ),
        (
            NativeType::declared("boolean", TypeFamily::Boolean),
            Primitive::Bool,
            Some("false"),
            Some(DefaultVal::Lit(Literal::Bool(false))),
        ),
        (
            decimal,
            Primitive::Decimal {
                precision: 16,
                scale: 3,
            },
            Some("10.125"),
            Some(DefaultVal::Lit(Literal::Decimal("10.125".into()))),
        ),
        (
            NativeType::declared("bytea", TypeFamily::Binary),
            Primitive::Bytes,
            None,
            None,
        ),
        (
            NativeType::declared("uuid", TypeFamily::Uuid),
            Primitive::Uuid,
            None,
            None,
        ),
        (
            NativeType::declared("jsonb", TypeFamily::Json),
            Primitive::Json,
            None,
            None,
        ),
        (
            NativeType::declared("date", TypeFamily::Date),
            Primitive::Date,
            None,
            None,
        ),
        (time, Primitive::Time, None, None),
        (timestamp, Primitive::Timestamp, None, None),
    ]
}
