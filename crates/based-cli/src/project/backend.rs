//! Construct database backends and redact their connection URLs.
#[cfg(any(feature = "mariadb", feature = "postgres"))]
use super::redact;
use crate::error::CliError;
use based_codegen::Dialect;

/// Build a single-shard [`based_runtime::Backend`] over `url` for the manifest dialect —
/// the same driver stack `based serve` uses (MariaDB/Postgres via a single-shard router;
/// SQLite over a file).
pub fn backend(dialect: Dialect, url: &str) -> Result<Box<dyn based_runtime::Backend>, CliError> {
    #[cfg(any(feature = "mariadb", feature = "postgres"))]
    use based_runtime::shard::PoolConfig;

    #[cfg(any(feature = "mariadb", feature = "postgres"))]
    let connecting = || format!("connecting to {}", redact(url));
    let backend: Box<dyn based_runtime::Backend> = match dialect {
        #[cfg(feature = "mariadb")]
        Dialect::MariaDb | Dialect::MySql => Box::new(
            based_runtime::driver::ShardRouter::single(url, PoolConfig::default())
                .map_err(|e| CliError::db(connecting(), e))?,
        ),
        #[cfg(feature = "postgres")]
        Dialect::Postgres => Box::new(
            based_runtime::PgRouter::single(url, PoolConfig::default())
                .map_err(|e| CliError::db(connecting(), e))?,
        ),
        #[cfg(not(feature = "mariadb"))]
        Dialect::MariaDb | Dialect::MySql => return Err(CliError::missing_driver("mariadb")),
        #[cfg(not(feature = "postgres"))]
        Dialect::Postgres => return Err(CliError::missing_driver("postgres")),
        // A SQLite `url` is a filesystem path (or `:memory:`, useless for a persisted apply).
        Dialect::Sqlite => Box::new(
            based_runtime::SqliteBackend::open(url)
                .map_err(|e| CliError::db(format!("opening {url}"), e))?,
        ),
    };
    Ok(backend)
}
