mod database;
#[allow(dead_code)]
mod client {
    include!("based_client.rs");
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let compiled = based_runtime::Compiled::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        .map_err(|error| format!("compile imported models: {error:?}"))?;
    let database = database::open(&std::env::var("DATABASE_URL")?).await?;
    let engine = based_runtime::Engine::new(compiled, database, based_runtime::id::UuidGen);
    let rows: Vec<client::LegacyRow> = client::embedded(&engine)
        .imported_entries(client::ImportedEntriesInput {}, ())
        .await?;
    println!("imported read: {}", serde_json::to_string(&rows)?);
    Ok(())
}
