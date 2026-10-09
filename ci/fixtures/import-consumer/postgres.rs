use sqlx::ConnectOptions;
pub async fn open(url: &str) -> Result<based_runtime::PgRouter, Box<dyn std::error::Error>> {
    let options = url
        .parse::<sqlx::postgres::PgConnectOptions>()?
        .disable_statement_logging();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    Ok(based_runtime::PgRouter::from_pool(pool))
}
