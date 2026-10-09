use based_catalog::{CatalogDialect, CatalogReadError, CatalogSource};
use sqlx::{
    postgres::{PgConnectOptions, PgConnection},
    ConnectOptions, Connection,
};

/// Connection options, including certificate-verified TLS, belong to the host.
/// Use an account with catalog access and schema USAGE, without application SELECT.
pub struct PostgresCatalogReader {
    pub(crate) connection: PgConnection,
    pub(crate) source: CatalogSource,
}

impl PostgresCatalogReader {
    /// Parse the host's SQLx URL options, preserving verified TLS and redacting failures.
    pub async fn connect_url(url: &str) -> Result<Self, CatalogReadError> {
        Self::connect(url.parse().map_err(|_| CatalogReadError::Connection)?).await
    }

    pub async fn connect(options: PgConnectOptions) -> Result<Self, CatalogReadError> {
        let mut connection = PgConnection::connect_with(&options.disable_statement_logging())
            .await
            .map_err(|_| CatalogReadError::Connection)?;
        let (version, database): (String, String) = sqlx::query_as(sqlx::AssertSqlSafe(
            "SELECT pg_catalog.current_setting('server_version'), pg_catalog.current_database()",
        ))
        .fetch_one(&mut connection)
        .await
        .map_err(|_| CatalogReadError::Metadata)?;
        Ok(Self {
            connection,
            source: CatalogSource {
                dialect: CatalogDialect::Postgres,
                server_version: version,
                database,
            },
        })
    }
}
