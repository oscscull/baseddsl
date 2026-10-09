use based_catalog::{CatalogDialect, CatalogReadError, CatalogSource};
use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Connection, SqliteConnection};
use std::path::Path;

pub struct SqliteCatalogReader {
    pub(crate) connection: SqliteConnection,
    pub(crate) source: CatalogSource,
}

impl SqliteCatalogReader {
    /// Open an existing database file, never an in-memory/new database or immutable live snapshot.
    pub async fn open(path: &Path) -> Result<Self, CatalogReadError> {
        if !path.is_file() {
            return Err(CatalogReadError::Connection);
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false)
            .pragma("query_only", "ON")
            .disable_statement_logging();
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .map_err(|_| CatalogReadError::Connection)?;
        super::collisions::require(&mut connection)
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        super::authorizer::install(&mut connection)
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        let version = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT sqlite_version()"))
            .fetch_one(&mut connection)
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        Ok(Self {
            connection,
            source: CatalogSource {
                dialect: CatalogDialect::Sqlite,
                server_version: version,
                database: path.file_name().map_or_else(
                    || "SQLite file".into(),
                    |name| name.to_string_lossy().into_owned(),
                ),
            },
        })
    }
    pub async fn close(self) -> Result<(), CatalogReadError> {
        self.connection
            .close()
            .await
            .map_err(|_| CatalogReadError::Metadata)
    }
}
