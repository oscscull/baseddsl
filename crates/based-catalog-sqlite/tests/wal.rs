//! Read the committed WAL catalog without an unsafe immutable-file shortcut.
#![cfg(feature = "sqlite")]
use based_catalog::{CatalogReader, Selection, TableId};
use based_catalog_sqlite::SqliteCatalogReader;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
    Connection, SqliteConnection,
};

#[tokio::test]
async fn live_wal_schema_is_visible_and_database_wal_bytes_are_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("live.db");
    let wal = directory.path().join("live.db-wal");
    let mut writer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal),
    )
    .await
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe("CREATE TABLE committed(id INTEGER PRIMARY KEY, label TEXT); INSERT INTO committed VALUES(1,'retained');"))
        .execute(&mut writer).await.unwrap();
    let before_main = std::fs::read(&path).unwrap();
    let before_wal = std::fs::read(&wal).unwrap();
    let mut reader = SqliteCatalogReader::open(&path).await.unwrap();
    let selected = Selection::new([TableId::new("main", "committed")]).unwrap();
    let discovery = reader.discover(&selected).await.unwrap();
    based_catalog::test_support::assert_supported(&discovery, &selected);
    assert_eq!(discovery.catalog.tables[0].columns.len(), 2);
    reader.close().await.unwrap();
    assert_eq!(before_main, std::fs::read(&path).unwrap());
    assert_eq!(before_wal, std::fs::read(&wal).unwrap());
    writer.close().await.unwrap();
}
