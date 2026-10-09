//! Explicit live gate: no skipping when infrastructure is requested.
#![cfg(feature = "mariadb")]
#[path = "support/assertions.rs"]
mod assertions;
#[path = "support/losses.rs"]
mod losses;
#[path = "support/ordering.rs"]
mod ordering;
#[path = "support/setup.rs"]
mod setup;
#[path = "support/snapshot.rs"]
mod snapshot;

use based_catalog::{CatalogCode, CatalogReader, Selection, TableId, ValueGeneration};
use based_catalog_mariadb::MariaDbCatalogReader;

#[tokio::test]
#[ignore = "requires TEST_MARIADB_URL; make ci-live-mariadb runs this gate"]
async fn independently_authored_catalog_is_metadata_only() {
    let fixture = setup::Fixture::create().await;
    let mut reader = MariaDbCatalogReader::connect(fixture.reader_options())
        .await
        .unwrap();
    let selection = selection(&["odd` table", "cycle_a", "cycle_b", "keyless"]);
    let discovery = reader.discover(&selection).await.unwrap();
    based_catalog::test_support::assert_supported(&discovery, &selection);
    assert!(discovery.catalog.source.server_version.starts_with("11.4."));
    eprintln!(
        "catalog reader evaluated: {} / InnoDB",
        discovery.catalog.source.server_version
    );
    assertions::physical_facts(&discovery);
    ordering::assert_arrival_independent(&discovery, &selection);
    let reverse = Selection::new(
        discovery
            .catalog
            .tables
            .iter()
            .rev()
            .map(|table| table.id.clone()),
    )
    .unwrap();
    assert_eq!(discovery, reader.discover(&reverse).await.unwrap());
    let outside = reader
        .discover(&self::selection(&["cycle_a"]))
        .await
        .unwrap();
    assert_eq!(outside.catalog.tables.len(), 1);
    assert!(outside
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::OutsideSelection));
    let missing = reader
        .discover(&self::selection(&["missing"]))
        .await
        .unwrap();
    assert!(missing
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::MissingTable));
    let unsupported = reader
        .discover(&self::selection(&["native_details", "selected_view"]))
        .await
        .unwrap();
    losses::losses(&unsupported);
    let native = &unsupported
        .catalog
        .tables
        .iter()
        .find(|table| table.id.name == "native_details")
        .unwrap()
        .columns;
    assert_eq!(native[0].generation, ValueGeneration::AutoIncrement);
    assert!(native[1].native_type.unsigned);
    assert!(matches!(
        native[3].generation,
        ValueGeneration::Generated { stored: true, .. }
    ));
    let mut partial =
        MariaDbCatalogReader::connect(fixture.reader_options().username("based_catalog_partial"))
            .await
            .unwrap();
    assert_eq!(
        partial
            .discover(&self::selection(&["cycle_a"]))
            .await
            .unwrap_err(),
        based_catalog::CatalogReadError::Metadata
    );
    fixture.verify_no_row_access_or_changes().await;
    fixture.remove().await;
}

fn selection(names: &[&str]) -> Selection {
    Selection::new(
        names
            .iter()
            .map(|name| TableId::new("based_catalog_fixture", *name)),
    )
    .unwrap()
}

#[tokio::test]
async fn connection_errors_are_redacted() {
    let options = "mysql://secret_user:secret_password@127.0.0.1:1/private_database"
        .parse()
        .unwrap();
    let Err(error) = MariaDbCatalogReader::connect(options).await else {
        panic!("unexpected connection");
    };
    let report = format!("{error:?}: {error}");
    for secret in ["secret_user", "secret_password", "private_database"] {
        assert!(!report.contains(secret));
    }
}
