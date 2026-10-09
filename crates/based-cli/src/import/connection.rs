//! Dispatch metadata readers without constructing an application/migration backend.
use crate::{error::CliError, local_config};
use based_catalog::{CatalogReader, Discovery, Selection};
use based_codegen::Dialect;
use std::path::Path;

pub(super) async fn discover(
    root: &Path,
    dialect: Dialect,
    explicit: Option<String>,
    selected: &Selection,
) -> Result<Discovery, CliError> {
    let urls = local_config::shard_urls(root, dialect, explicit.into_iter().collect())?;
    if urls.len() != 1 {
        return Err(CliError::usage(
            "one-shot import requires exactly one database connection, not a shard list",
        ));
    }
    let url = &urls[0];
    let result = match dialect {
        Dialect::Sqlite => {
            let mut reader = based_catalog_sqlite::SqliteCatalogReader::open(Path::new(url))
                .await
                .map_err(error)?;
            let discovery = reader.discover(selected).await;
            reader.close().await.map_err(error)?;
            discovery
        }
        #[cfg(feature = "mariadb")]
        Dialect::MariaDb | Dialect::MySql => {
            based_catalog_mariadb::MariaDbCatalogReader::connect_url(url)
                .await
                .map_err(error)?
                .discover(selected)
                .await
        }
        #[cfg(feature = "postgres")]
        Dialect::Postgres => {
            based_catalog_postgres::PostgresCatalogReader::connect_url(url)
                .await
                .map_err(error)?
                .discover(selected)
                .await
        }
        #[cfg(not(feature = "mariadb"))]
        Dialect::MariaDb | Dialect::MySql => return Err(CliError::missing_driver("mariadb")),
        #[cfg(not(feature = "postgres"))]
        Dialect::Postgres => return Err(CliError::missing_driver("postgres")),
    };
    result.map_err(error)
}

fn error(error: based_catalog::CatalogReadError) -> CliError {
    CliError::failure(format!("metadata-only import failed: {error}; verify the selected objects, catalog visibility and connection/TLS configuration (connection value redacted)"))
}
