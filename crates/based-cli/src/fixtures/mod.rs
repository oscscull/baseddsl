//! `based fixtures`: a test-fixture process. It reads one JSON request per stdin line,
//! `{"model": "...", "row": {...}}`, inserts the row through the engine and answers on
//! stdout with `{"row": {...}}` or `{"error": {"code", "message"}}`, in order. Generated
//! TypeScript fixtures drive it; it opens no socket.

mod lines;

use crate::error::CliError;
use crate::local_config::shard_urls;
use crate::project::load_checked;
use based_codegen::Dialect;
use based_runtime::{Compiled, Engine, GuardVerdict, Guards};
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct FixturesOptions {
    /// Explicit project root; otherwise find the nearest ancestor based.toml.
    pub root: Option<PathBuf>,
    /// The database to insert into. Falls back to `BASED_DATABASE_URL`.
    #[arg(long = "database-url")]
    pub database_url: Vec<String>,
}

pub async fn cmd_fixtures(root: &Path, options: FixturesOptions) -> Result<(), CliError> {
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let urls = shard_urls(root, dialect, options.database_url)?;
    let compiled = Compiled::from_checked(schema, decls, dialect);
    let guards = refuse_guards(&compiled);
    #[cfg(any(feature = "mariadb", feature = "postgres"))]
    let pool = based_runtime::shard::PoolConfig::default();
    match dialect {
        #[cfg(feature = "mariadb")]
        Dialect::MariaDb | Dialect::MySql => {
            let router = based_runtime::driver::ShardRouter::new(&urls, pool)
                .map_err(|e| CliError::db("connecting to database", e))?;
            lines::serve(engine(compiled, router, guards)?).await
        }
        #[cfg(feature = "postgres")]
        Dialect::Postgres => {
            let router = based_runtime::PgRouter::new(&urls, pool)
                .map_err(|e| CliError::db("connecting to database", e))?;
            lines::serve(engine(compiled, router, guards)?).await
        }
        #[cfg(not(feature = "mariadb"))]
        Dialect::MariaDb | Dialect::MySql => Err(CliError::missing_driver("mariadb")),
        #[cfg(not(feature = "postgres"))]
        Dialect::Postgres => Err(CliError::missing_driver("postgres")),
        Dialect::Sqlite => {
            let [url] = urls.as_slice() else {
                return Err(CliError::usage(format!(
                    "sqlite uses a single database file, got {} urls",
                    urls.len()
                )));
            };
            let backend = based_runtime::SqliteBackend::open(url)
                .map_err(|e| CliError::db(format!("opening {url}"), e))?;
            lines::serve(engine(compiled, backend, guards)?).await
        }
    }
}

fn engine(
    compiled: Compiled,
    backend: impl based_runtime::Backend + 'static,
    guards: Guards,
) -> Result<Engine, CliError> {
    Engine::with_guards(compiled, backend, based_runtime::id::UuidGen, guards)
        .map_err(|e| CliError::caused_by("building the engine", e))
}

/// Fixtures run no mutations, so every declared guard is registered as a refusal.
fn refuse_guards(compiled: &Compiled) -> Guards {
    compiled
        .declared_guards()
        .fold(Guards::new(), |guards, (_, guard)| {
            guards.register(guard, |_| async {
                GuardVerdict::deny("the fixtures process runs no guarded mutations")
            })
        })
}
