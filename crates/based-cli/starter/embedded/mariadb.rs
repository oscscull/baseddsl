//! Connect to the caller-provided MariaDB server; never provision a server.
pub fn connect(
    _: &std::path::Path,
) -> Result<based_runtime::driver::ShardRouter, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| "set DATABASE_URL for your existing MariaDB database")?;
    Ok(based_runtime::driver::ShardRouter::new(
        &[url],
        based_runtime::shard::PoolConfig::default(),
    )?)
}
