//! `based serve`: stand the checked schema up as a live RPC service over the sharded
//! connection pool, with graceful drain on SIGTERM/SIGINT.

use crate::error::CliError;
use crate::local_config::shard_urls;
use crate::project::load_checked;
use based_codegen::Dialect;
use std::path::Path;

/// `based serve`: stand the checked schema up as a live RPC service. Runs the same
/// front end as every other command (rendering diagnostics, bailing on any error —
/// a dirty schema never serves), builds the sharded connection pool, and hands both to
/// the runtime's HTTP listener. Blocks until the process is killed.
pub async fn cmd_serve(
    root: &Path,
    listen: &str,
    database_url: Vec<String>,
    pool_min: usize,
    pool_max: usize,
) -> Result<(), CliError> {
    use based_runtime::http::{ServeConfig, TrustedHeaderContext};
    #[cfg(any(feature = "mariadb", feature = "postgres"))]
    use based_runtime::shard::PoolConfig;
    use based_runtime::Compiled;

    if pool_max == 0 || pool_min > pool_max {
        return Err(CliError::usage(
            "invalid pool options: --pool-max must be positive and --pool-min must not exceed it",
        ));
    }

    // Reuse the shared front end so diagnostics render exactly as `based check` does,
    // then build the served artifact from the clean schema (no second parse/check).
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let urls = shard_urls(root, dialect, database_url)?;
    let compiled = Compiled::from_checked(schema, decls, dialect);

    // Pool sizing from the flags; the hardening timeouts (checkout + statement) keep
    // their conservative defaults (a saturated pool → fast 503, a runaway query
    // aborted). The pool is also the concurrency ceiling — requests past it wait at
    // most the checkout timeout, then fail fast.
    #[cfg(any(feature = "mariadb", feature = "postgres"))]
    let pool = PoolConfig {
        min: pool_min,
        max: pool_max,
        ..PoolConfig::default()
    };
    #[cfg(not(any(feature = "mariadb", feature = "postgres")))]
    let _ = (pool_min, pool_max);
    let config = ServeConfig {
        listen: listen.to_string(),
    };

    eprintln!("based serve: {dialect:?}, listening on {listen}");
    eprintln!("liveness: GET /healthz  readiness: GET /readyz");

    // Build the backend for the manifest dialect and stand the listener up. The `@scope`
    // owner field routes to a shard schema-side, so no shard key is hand-set here —
    // the driver reads it off the compiled schema. SQLite is a single local file (one url,
    // one shared database), so it neither shards nor pools.
    let ctx = TrustedHeaderContext;
    match dialect {
        #[cfg(feature = "mariadb")]
        Dialect::MariaDb | Dialect::MySql => {
            let router = based_runtime::driver::ShardRouter::new(&urls, pool)
                .map_err(|e| CliError::db("connecting to database", e))?;
            run_listener(compiled, router, ctx, config).await
        }
        #[cfg(feature = "postgres")]
        Dialect::Postgres => {
            let router = based_runtime::PgRouter::new(&urls, pool)
                .map_err(|e| CliError::db("connecting to database", e))?;
            run_listener(compiled, router, ctx, config).await
        }
        #[cfg(not(feature = "mariadb"))]
        Dialect::MariaDb | Dialect::MySql => Err(CliError::missing_driver("mariadb")),
        #[cfg(not(feature = "postgres"))]
        Dialect::Postgres => Err(CliError::missing_driver("postgres")),
        Dialect::Sqlite => {
            if urls.len() > 1 {
                return Err(CliError::usage(format!(
                    "sqlite serves a single database file, got {} urls",
                    urls.len()
                )));
            }
            let backend = based_runtime::SqliteBackend::open(&urls[0])
                .map_err(|e| CliError::db(format!("opening {}", urls[0]), e))?;
            run_listener(compiled, backend, ctx, config).await
        }
    }
}

/// Stand the listener up over a concrete backend and block until the process is signalled.
/// Generic over the backend so each dialect's driver drops in without the listener naming
/// it. Graceful shutdown: on SIGTERM/SIGINT begin draining (readiness fails first, so a
/// load balancer pulls this instance out of rotation) and let in-flight requests finish,
/// then the call returns. The handle is captured once the listener is up (`on_start`), so
/// the signal handler can only fire after we're serving.
async fn run_listener(
    compiled: based_runtime::Compiled,
    backend: impl based_runtime::Backend + 'static,
    ctx: based_runtime::http::TrustedHeaderContext,
    config: based_runtime::http::ServeConfig,
) -> Result<(), CliError> {
    based_runtime::http::serve_with_handle(compiled, backend, ctx, config, |handle| {
        if let Err(e) = ctrlc::set_handler(move || {
            eprintln!("based serve: shutdown signal received, draining…");
            handle.shutdown();
        }) {
            // A missing signal handler is non-fatal — the server still runs, it just
            // can't drain gracefully (a hard kill still stops it).
            eprintln!("based serve: could not install shutdown handler: {e}");
        }
    })
    .await
    .map_err(|e| CliError::caused_by("serve failed", e))
}
