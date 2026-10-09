use sqlx::ConnectOptions;
pub async fn open(
    url: &str,
) -> Result<based_runtime::driver::ShardRouter, Box<dyn std::error::Error>> {
    let options = url
        .parse::<sqlx::mysql::MySqlConnectOptions>()?
        .disable_statement_logging();
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    Ok(based_runtime::driver::ShardRouter::from_pool(pool))
}
