//! Independent InnoDB fixture. CREATE refuses pre-existing objects; cleanup runs only after success.
use sqlx::{mysql::MySqlConnectOptions, Connection, MySqlConnection, Row};

pub async fn verify() {
    let url = std::env::var("TEST_MARIADB_URL").expect("TEST_MARIADB_URL is required");
    let options: MySqlConnectOptions = url.parse().unwrap();
    let mut admin = MySqlConnection::connect_with(&options).await.unwrap();
    // No initial DROP: a collision fails setup and never deletes a shared object.
    sqlx::raw_sql(sqlx::AssertSqlSafe(include_str!("mariadb.sql")))
        .execute(&mut admin)
        .await
        .unwrap();
    let metadata = options
        .clone()
        .username("based_import_metadata")
        .password("fixture_metadata_only")
        .database("based_import_fixture");
    let before = snapshot(&mut admin).await;
    let mut reader = MySqlConnection::connect_with(&metadata).await.unwrap();
    for statement in [
        "SELECT * FROM based_import_fixture.legacy_entry",
        "INSERT INTO based_import_fixture.legacy_account VALUES ('forbidden','forbidden')",
        "ALTER TABLE based_import_fixture.legacy_entry ADD COLUMN forbidden INT",
    ] {
        assert!(sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut reader)
            .await
            .is_err());
    }
    reader.close().await.unwrap();
    super::server::verify("mariadb");
    assert_eq!(
        before,
        snapshot(&mut admin).await,
        "import/typed read changed original schema or data"
    );
    // Every CREATE succeeded before this cleanup; no pre-existing objects are removed.
    sqlx::raw_sql(sqlx::AssertSqlSafe("DROP DATABASE based_import_fixture; DROP USER based_import_metadata@'%'; DROP USER based_import_consumer@'%';"))
        .execute(&mut admin).await.unwrap();
}

async fn snapshot(admin: &mut MySqlConnection) -> Vec<String> {
    let tables: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA='based_import_fixture' ORDER BY TABLE_NAME"))
        .fetch_all(&mut *admin).await.unwrap();
    let mut facts = tables.clone();
    for table in tables {
        let statement = format!(
            "SHOW CREATE TABLE based_import_fixture.`{}`",
            table.replace('`', "``")
        );
        facts.push(
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .fetch_one(&mut *admin)
                .await
                .unwrap()
                .get(1),
        );
    }
    for statement in [
        "SELECT CONCAT(account_code,':',label) FROM based_import_fixture.legacy_account ORDER BY account_code",
        "SELECT CONCAT(entry_key,':',parent_code,':',heading) FROM based_import_fixture.legacy_entry ORDER BY entry_key",
    ] {
        facts.extend(sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(statement)).fetch_all(&mut *admin).await.unwrap());
    }
    facts
}
