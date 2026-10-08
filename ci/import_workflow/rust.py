"""Render the minimal typed reader and caller-owned read-only backend for each dialect."""
READS = """shape LegacyRow from LegacyEntry { heading parent_code { label } }
query imported_entries() -> LegacyRow[] { list LegacyEntry order (entry_key asc); }
"""

MAIN = """mod database;
#[allow(dead_code)]
mod client { include!("based_client.rs"); }
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let compiled = based_runtime::Compiled::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        .map_err(|error| format!("compile imported models: {error:?}"))?;
    let database = database::open(&std::env::var("DATABASE_URL")?).await?;
    let engine = based_runtime::Engine::new(compiled, database, based_runtime::id::UuidGen);
    let rows: Vec<client::LegacyRow> = client::embedded(&engine)
        .imported_entries(client::ImportedEntriesInput {}, ()).await?;
    println!("imported read: {}", serde_json::to_string(&rows)?);
    Ok(())
}
"""


def manifest(dialect, commit):
    driver = {"sqlite": "sqlite", "mariadb": "mysql", "postgres": "postgres"}[dialect]
    tls = ', "tls-rustls"' if dialect != "sqlite" else ""
    sqlx_tls = ', "tls-rustls-ring-webpki"' if dialect != "sqlite" else ""
    return f"""[package]
name = "based-import-consumer"
version = "0.1.0"
edition = "2021"
rust-version = "1.94"
[workspace]
[dependencies]
based-runtime = {{ git = "https://github.com/oscscull/baseddsl.git", rev = "{commit}", features = ["{dialect}", "id-gen"{tls}] }}
sqlx = {{ version = "0.9.0", default-features = false, features = ["runtime-tokio", "{driver}"{sqlx_tls}] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"""


def backend(dialect):
    if dialect == "sqlite":
        return """use sqlx::ConnectOptions;
pub async fn open(path: &str) -> Result<based_runtime::SqliteBackend, Box<dyn std::error::Error>> {
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(path)
        .read_only(true).create_if_missing(false).pragma("query_only", "ON")
        .disable_statement_logging();
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(options).await?;
    Ok(based_runtime::SqliteBackend::from_pool(pool))
}
"""
    options, pool, router = {
        "postgres": ("postgres::PgConnectOptions", "postgres::PgPoolOptions", "PgRouter"),
        "mariadb": ("mysql::MySqlConnectOptions", "mysql::MySqlPoolOptions", "driver::ShardRouter"),
    }[dialect]
    return f"""use sqlx::ConnectOptions;
pub async fn open(url: &str) -> Result<based_runtime::{router}, Box<dyn std::error::Error>> {{
    let options = url.parse::<sqlx::{options}>()?.disable_statement_logging();
    let pool = sqlx::{pool}::new().max_connections(1).connect_with(options).await?;
    Ok(based_runtime::{router}::from_pool(pool))
}}
"""
