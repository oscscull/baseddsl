//! Hand-authored file proof, included in the existing SQLite workspace feature tier.
#![cfg(feature = "sqlite")]
#[path = "support/assertions.rs"]
mod assertions;
#[path = "support/losses.rs"]
mod losses;
#[path = "support/setup.rs"]
mod setup;

use based_catalog::{CatalogCode, CatalogReader, Selection, TableId};
use based_catalog_sqlite::SqliteCatalogReader;

#[tokio::test]
async fn independently_authored_file_is_discovered_without_row_reads_or_writes() {
    let fixture = setup::Fixture::create().await;
    let mut reader = SqliteCatalogReader::open(&fixture.path).await.unwrap();
    let selected = selection(&[
        "odd\" table",
        "cycle_a",
        "cycle_b",
        "keyless",
        "rowid_auto",
        "rowid_reuse",
        "strict_values",
    ]);
    let discovery = reader.discover(&selected).await.unwrap();
    based_catalog::test_support::assert_supported(&discovery, &selected);
    eprintln!(
        "catalog reader evaluated: SQLite {}",
        discovery.catalog.source.server_version
    );
    assertions::physical_facts(&discovery);
    let outside = reader.discover(&selection(&["cycle_a"])).await.unwrap();
    assert_eq!(outside.catalog.tables.len(), 1);
    assert!(outside
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::OutsideSelection));
    let missing = reader.discover(&selection(&["missing"])).await.unwrap();
    assert!(missing
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::MissingTable));
    let unsupported = reader
        .discover(&selection(&[
            "native_details",
            "selected_view",
            "untyped",
            "descending_pk",
            "foreign_semantics",
            "rowid_reuse",
        ]))
        .await
        .unwrap();
    losses::limitations(&unsupported);
    reader.close().await.unwrap();
    fixture.assert_unchanged();
}

fn selection(names: &[&str]) -> Selection {
    Selection::new(names.iter().map(|name| TableId::new("main", *name))).unwrap()
}

#[tokio::test]
async fn missing_files_are_not_created_and_paths_are_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("private_secret_database.db");
    let Err(error) = SqliteCatalogReader::open(&missing).await else {
        panic!("missing file accepted");
    };
    assert!(!format!("{error:?}: {error}").contains("private_secret_database"));
    assert!(!missing.exists());
}
