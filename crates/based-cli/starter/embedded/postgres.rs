//! Connect to the caller-provided PostgreSQL server; never provision a server.
pub fn connect(_: &std::path::Path) -> Result<based_runtime::PgRouter, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| "set DATABASE_URL for your existing PostgreSQL database")?;
    Ok(based_runtime::PgRouter::new(
        &[url],
        based_runtime::shard::PoolConfig::default(),
    )?)
}
