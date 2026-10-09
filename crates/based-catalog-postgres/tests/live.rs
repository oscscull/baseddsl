//! Explicit live proof; no infrastructure skip when requested by the existing PostgreSQL tier.
#![cfg(feature = "postgres")]
#[path = "support/assertions.rs"]
mod assertions;
#[path = "support/native.rs"]
mod native;
#[path = "support/setup.rs"]
mod setup;
#[path = "support/snapshot.rs"]
mod snapshot;

use based_catalog::{CatalogCode, CatalogReader, Selection, TableId};
use based_catalog_postgres::PostgresCatalogReader;

#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL; make ci-live-postgres runs this gate"]
async fn independently_authored_catalog_is_metadata_only() {
    let fixture = setup::Fixture::create().await;
    let mut reader = PostgresCatalogReader::connect(fixture.reader_options())
        .await
        .unwrap();
    let selected = selection(&["odd\" table", "cycle_a", "keyless"], true);
    let discovery = reader.discover(&selected).await.unwrap();
    based_catalog::test_support::assert_supported(&discovery, &selected);
    assert!(discovery.catalog.source.server_version.starts_with("16."));
    eprintln!(
        "catalog reader evaluated: PostgreSQL {}",
        discovery.catalog.source.server_version
    );
    assertions::physical_facts(&discovery);
    let outside = reader
        .discover(&selection(&["cycle_a"], false))
        .await
        .unwrap();
    assert_eq!(outside.catalog.tables.len(), 1);
    assert!(outside
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::OutsideSelection));
    let missing = reader
        .discover(&selection(&["missing"], false))
        .await
        .unwrap();
    assert!(missing
        .diagnostics
        .iter()
        .any(|finding| finding.code == CatalogCode::MissingTable));
    let unsupported = reader
        .discover(&selection(
            &[
                "native_details",
                "selected_view",
                "foreign_semantics",
                "odd\" table",
            ],
            false,
        ))
        .await
        .unwrap();
    native::native_facts(&unsupported);
    native::foreign_semantics(&unsupported);
    let hidden = Selection::new([TableId::new("catalog_hidden", "private_table")]).unwrap();
    assert_eq!(
        reader.discover(&hidden).await.unwrap_err(),
        based_catalog::CatalogReadError::Metadata
    );
    fixture.verify_no_row_access_or_changes().await;
    fixture.remove().await;
}

fn selection(names: &[&str], include_cycle_target: bool) -> Selection {
    let target = include_cycle_target.then(|| TableId::new("catalog_other", "cycle_b"));
    Selection::new(
        names
            .iter()
            .map(|name| TableId::new("catalog fixture", *name))
            .chain(target),
    )
    .unwrap()
}

#[tokio::test]
async fn connection_errors_are_redacted() {
    let options = "postgres://secret_user:secret_password@127.0.0.1:1/private_database"
        .parse()
        .unwrap();
    let Err(error) = PostgresCatalogReader::connect(options).await else {
        panic!("unexpected connection");
    };
    let report = format!("{error:?}: {error}");
    for secret in ["secret_user", "secret_password", "private_database"] {
        assert!(!report.contains(secret));
    }
}
