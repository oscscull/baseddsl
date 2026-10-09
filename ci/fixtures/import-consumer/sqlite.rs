use sqlx::ConnectOptions;
pub async fn open(path: &str) -> Result<based_runtime::SqliteBackend, Box<dyn std::error::Error>> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .pragma("query_only", "ON")
        .disable_statement_logging();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    Ok(based_runtime::SqliteBackend::from_pool(pool))
}
