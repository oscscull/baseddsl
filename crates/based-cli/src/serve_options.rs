//! Parse standalone listener options. Runtime setup lives in serve.rs.
use crate::idempotency_store;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct ServeOptions {
    /// Explicit project root; otherwise find the nearest ancestor based.toml.
    pub root: Option<PathBuf>,
    /// Address to bind the HTTP listener on. `BASED_LISTEN` overrides the default —
    /// a container sets `0.0.0.0:8080` there so the port is reachable from outside.
    #[arg(long, env = "BASED_LISTEN", default_value = "127.0.0.1:8080")]
    pub listen: String,
    /// A database URL per physical shard (repeat for a sharded fleet). Falls back
    /// to `BASED_DATABASE_URL` (comma-separated) when none is passed.
    #[arg(long = "database-url")]
    pub database_url: Vec<String>,
    /// Warm connections kept per shard pool.
    #[arg(long, default_value_t = 4)]
    pub pool_min: usize,
    /// Max database connections per shard. Bound HTTP concurrency at the trusted edge.
    #[arg(long, default_value_t = 32)]
    pub pool_max: usize,
    #[command(flatten)]
    pub idempotency: idempotency_store::StoreOptions,
    #[command(flatten)]
    pub guards: GuardOptions,
}

#[derive(Default, clap::Args)]
pub struct GuardOptions {
    /// TOML guard mappings (relative to project root). Secrets come from named env vars.
    #[arg(long)]
    pub guard_config: Option<PathBuf>,
    /// Development only: allow HTTP callbacks on literal loopback hosts.
    #[arg(long)]
    pub guard_allow_loopback_http: bool,
}
