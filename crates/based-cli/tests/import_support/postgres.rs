//! Independently authored fixture. CREATE refuses pre-existing objects; cleanup runs only after success.
use sqlx::{postgres::PgConnectOptions, Connection, PgConnection};

pub async fn verify() {
    let url = std::env::var("TEST_POSTGRES_URL").expect("TEST_POSTGRES_URL is required");
    let options: PgConnectOptions = url.parse().unwrap();
    let mut admin = PgConnection::connect_with(&options).await.unwrap();
    // No initial DROP: an existing schema or account makes setup fail without deleting it.
    sqlx::raw_sql(sqlx::AssertSqlSafe(include_str!("postgres.sql")))
        .execute(&mut admin)
        .await
        .unwrap();
    let metadata = options
        .clone()
        .username("based_import_metadata")
        .password("fixture_metadata_only");
    let before = snapshot(&mut admin).await;
    let mut reader = PgConnection::connect_with(&metadata).await.unwrap();
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
    super::server::verify("postgres");
    assert_eq!(
        before,
        snapshot(&mut admin).await,
        "import/typed read changed original schema or data"
    );
    // Reached only after every CREATE succeeded: these objects belong to this invocation.
    sqlx::raw_sql(sqlx::AssertSqlSafe("DROP SCHEMA based_import_fixture CASCADE; DROP ROLE based_import_metadata; DROP ROLE based_import_consumer;"))
        .execute(&mut admin).await.unwrap();
}

async fn snapshot(admin: &mut PgConnection) -> Vec<String> {
    let mut facts = Vec::new();
    for statement in [
        "SELECT c.relname::text || ':' || a.attname::text || ':' || pg_catalog.format_type(a.atttypid,a.atttypmod) || ':' || a.attnotnull::text || ':' || COALESCE(pg_catalog.pg_get_expr(d.adbin,d.adrelid,false),'') FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE n.nspname='based_import_fixture' AND a.attnum > 0 AND NOT a.attisdropped ORDER BY c.relname,a.attnum",
        "SELECT c.conname::text || ':' || pg_catalog.pg_get_constraintdef(c.oid,false) FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_namespace n ON n.oid=c.connamespace WHERE n.nspname='based_import_fixture' ORDER BY c.conname",
        "SELECT pg_catalog.pg_get_indexdef(c.oid,0,false) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='based_import_fixture' AND c.relkind='i' ORDER BY c.relname",
        "SELECT account_code || ':' || label FROM based_import_fixture.legacy_account ORDER BY account_code",
        "SELECT entry_key::text || ':' || parent_code || ':' || heading FROM based_import_fixture.legacy_entry ORDER BY entry_key",
        "SELECT c.relname::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='based_import_fixture' ORDER BY c.relname",
    ] {
        facts.extend(sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(statement)).fetch_all(&mut *admin).await.unwrap());
    }
    facts
}
