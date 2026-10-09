//! Explicit live import proof; CI requests these tests rather than silently skipping infrastructure.
#[cfg(feature = "mariadb")]
#[path = "import_support/mariadb.rs"]
mod mariadb;
#[cfg(feature = "postgres")]
#[path = "import_support/postgres.rs"]
mod postgres;
#[cfg(any(feature = "postgres", feature = "mariadb"))]
#[path = "import_support/server.rs"]
mod server;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_POSTGRES_URL; make ci-live-postgres requests this proof"]
async fn postgres_import_reads_original_database_without_adopting_migrations() {
    postgres::verify().await;
}

#[cfg(feature = "mariadb")]
#[tokio::test]
#[ignore = "requires TEST_MARIADB_URL; make ci-live-mariadb requests this proof"]
async fn mariadb_import_reads_original_database_without_adopting_migrations() {
    mariadb::verify().await;
}
