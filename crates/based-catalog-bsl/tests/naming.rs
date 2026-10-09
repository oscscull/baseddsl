#[path = "support/fixture.rs"]
mod fixture;
use based_catalog::*;
use based_catalog_bsl::emit;

#[test]
fn portable_model_collisions_reserved_fields_and_unicode_keep_original_aliases() {
    let mut catalog = fixture::catalog(CatalogDialect::Sqlite);
    catalog.tables.clear();
    for name in ["order-item", "order item", "ORDER_item", "text", "123", "Å"] {
        catalog.tables.push(fixture::table(
            "main",
            name,
            vec![
                fixture::column("Part A", 0, "TEXT", TypeFamily::Text, true),
                fixture::column("part-a", 1, "TEXT", TypeFamily::Text, true),
                fixture::column("query", 2, "TEXT", TypeFamily::Text, true),
                fixture::column("é", 3, "TEXT", TypeFamily::Text, true),
            ],
            &[],
        ));
    }
    let (discovery, selection) = fixture::discovery(catalog);
    let output = emit(
        &discovery,
        &selection,
        &fixture::manifest(CatalogDialect::Sqlite, "none"),
    )
    .unwrap();
    let first = &output.names[&TableId::new("main", "order item")];
    assert_eq!(first.fields["Part A"], "part_a");
    assert_eq!(first.fields["part-a"], "part_a_2");
    assert_eq!(first.fields["query"], "imported_query");
    assert_eq!(first.fields["é"], "ue9");
    let models: std::collections::BTreeSet<_> = output
        .names
        .values()
        .map(|name| name.model.clone())
        .collect();
    assert!(models.contains("OrderItem"));
    assert!(models.contains("OrderItem2"));
    assert!(models.contains("OrderItem3"));
    assert!(models.contains("ImportedText"));
    assert!(models.contains("Imported123"));
    assert!(models.contains("Uc5"));
    let files: std::collections::BTreeSet<_> = output
        .files
        .iter()
        .map(|(path, _)| path.to_string_lossy().to_ascii_lowercase())
        .collect();
    assert_eq!(files.len(), output.files.len());
    for table in &discovery.catalog.tables {
        let model = output
            .checked
            .schema
            .model(&output.names[&table.id].model)
            .unwrap();
        assert_eq!(model.table, table.id.name);
        assert_eq!(
            model
                .members
                .iter()
                .map(based_sema::RMember::physical_col)
                .collect::<Vec<_>>(),
            ["Part A", "part-a", "query", "é"]
        );
    }
}
