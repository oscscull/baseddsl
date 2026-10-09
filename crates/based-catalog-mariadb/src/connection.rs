use based_catalog::{CatalogDialect, CatalogReadError, CatalogSource};
use sqlx::{
    mysql::{MySqlConnectOptions, MySqlConnection},
    ConnectOptions, Connection,
};

/// Owns one metadata connection. Connection options (including verified TLS) belong to the host.
/// Use a REFERENCES-only account; discovery additionally enforces a read-only transaction.
pub struct MariaDbCatalogReader {
    pub(crate) connection: MySqlConnection,
    pub(crate) source: CatalogSource,
}

impl MariaDbCatalogReader {
    /// Parse the host's SQLx URL options, preserving verified TLS and redacting failures.
    pub async fn connect_url(url: &str) -> Result<Self, CatalogReadError> {
        Self::connect(url.parse().map_err(|_| CatalogReadError::Connection)?).await
    }

    pub async fn connect(options: MySqlConnectOptions) -> Result<Self, CatalogReadError> {
        let mut connection = MySqlConnection::connect_with(&options.disable_statement_logging())
            .await
            .map_err(|_| CatalogReadError::Connection)?;
        let (version, database): (String, Option<String>) =
            sqlx::query_as(sqlx::AssertSqlSafe("SELECT VERSION(), DATABASE()"))
                .fetch_one(&mut connection)
                .await
                .map_err(|_| CatalogReadError::Metadata)?;
        if !version.contains("MariaDB") {
            return Err(CatalogReadError::Metadata);
        }
        Ok(Self {
            connection,
            source: CatalogSource {
                dialect: CatalogDialect::MariaDb,
                server_version: version,
                database: database.unwrap_or_default(),
            },
        })
    }
}
