//! Prove the actual owned connection rejects row probes and writes, not just that queries look safe.
use super::SqliteCatalogReader;
use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};

#[tokio::test]
async fn owned_connection_denies_rows_counts_and_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let mut admin = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe("CREATE TABLE private_rows(value TEXT); INSERT INTO private_rows VALUES('private fixture');"))
        .execute(&mut admin).await.unwrap();
    admin.close().await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut reader = SqliteCatalogReader::open(&path).await.unwrap();
    for sql in [
        "SELECT value FROM private_rows",
        "SELECT count(*) FROM private_rows",
        "INSERT INTO private_rows VALUES('forbidden')",
        "ALTER TABLE private_rows ADD COLUMN forbidden INT",
        "CREATE TEMP TABLE forbidden(value TEXT)",
    ] {
        assert!(
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut reader.connection)
                .await
                .is_err(),
            "allowed {sql}"
        );
    }
    reader.connection.close().await.unwrap();
    assert_eq!(before, std::fs::read(&path).unwrap());
}

#[tokio::test]
async fn a_real_table_cannot_shadow_metadata_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collision.db");
    let mut admin = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(
        "CREATE TABLE pragma_table_xinfo(private_value TEXT)",
    ))
    .execute(&mut admin)
    .await
    .unwrap();
    admin.close().await.unwrap();
    let Err(error) = SqliteCatalogReader::open(&path).await else {
        panic!("shadowed metadata table was accepted");
    };
    assert_eq!(error, based_catalog::CatalogReadError::Metadata);
}
