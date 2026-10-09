#[path = "support/fixture.rs"]
mod fixture;
use based_catalog::{CatalogDialect, Selection};
use based_catalog_bsl::emit;
use based_sema::MemberKind;

#[test]
fn three_dialects_preserve_independent_physical_keys_defaults_relations_and_indexes() {
    for dialect in [
        CatalogDialect::Sqlite,
        CatalogDialect::Postgres,
        CatalogDialect::MariaDb,
    ] {
        let (discovery, selection) = fixture::discovery(fixture::catalog(dialect));
        for convention in ["none", "all"] {
            let output = emit(
                &discovery,
                &selection,
                &fixture::manifest(dialect, convention),
            )
            .unwrap();
            assert!(!output.report.has_errors());
            let parent = output.checked.schema.model("OddTable").unwrap();
            assert_eq!(parent.table, "odd\" table");
            assert_eq!(parent.pk_columns(), ["odd id"]);
            assert!(parent.pk_is_db_generated());
            let MemberKind::Scalar { default, .. } = &parent.member("label").unwrap().kind else {
                panic!()
            };
            assert_eq!(
                default,
                &Some(based_ast::DefaultVal::Lit(based_ast::Literal::Str(
                    "it's retained".into()
                )))
            );
            let child = output.checked.schema.model("Child").unwrap();
            assert_eq!(child.pk_columns(), ["part z", "part a"]);
            assert_eq!(child.indexes[0].columns, ["part_a", "part_z"]);
            let parent_ref = child.member("parent_ref").unwrap();
            assert_eq!(
                output.checked.schema.fk_columns(parent_ref)[0].0,
                "parent ref"
            );
            assert_eq!(
                output.checked.schema.fk_columns(parent_ref)[0]
                    .1
                    .physical_col(),
                "odd id"
            );
            let keyless = output.checked.schema.model("Keyless").unwrap();
            assert!(keyless.no_id);
            assert!(matches!(
                keyless.member("owner_id").unwrap().kind,
                MemberKind::Scalar { .. }
            ));
            assert!(keyless.created.is_none());
            assert!(output.checked.schema.scopes.is_empty());
            assert!(output.checked.schema.queries.is_empty());
            assert!(output.checked.schema.mutations.is_empty());
            for (_, source) in &output.files {
                assert!(based_fmt::is_formatted(source).unwrap());
            }
            let repeat = emit(
                &discovery,
                &selection,
                &fixture::manifest(dialect, convention),
            )
            .unwrap();
            assert_eq!(output.files, repeat.files);
            let mut arrival = discovery.clone();
            arrival.catalog.tables.reverse();
            for table in &mut arrival.catalog.tables {
                table.columns.reverse();
                table.indexes.reverse();
                table.foreign_keys.reverse();
            }
            let reversed = emit(
                &arrival,
                &selection,
                &fixture::manifest(dialect, convention),
            )
            .unwrap();
            assert_eq!(output.files, reversed.files);
            assert_eq!(output.names, reversed.names);
            assert!(output
                .checked
                .project
                .files
                .iter()
                .all(|file| file.path.starts_with("models")));
        }
        let parent_only = Selection::new([discovery
            .catalog
            .tables
            .iter()
            .find(|table| table.id.name == "cycle_a")
            .unwrap()
            .id
            .clone()])
        .unwrap();
        let partial =
            based_catalog::Discovery::checked(discovery.catalog.clone(), &parent_only, Vec::new());
        let Err(report) = emit(&partial, &parent_only, &fixture::manifest(dialect, "none")) else {
            panic!("partial selection accepted")
        };
        assert!(report.has_errors());
    }
}
