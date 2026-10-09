//! Create and read an item through the generated in-process client.
mod database;

#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let compiled = based_runtime::Compiled::load(root).map_err(|e| format!("schema: {e:?}"))?;
    let engine = based_runtime::Engine::new(
        compiled,
        database::connect(root)?,
        based_runtime::id::UuidGen,
    );
    let api = client::embedded(&engine);
    let created = api
        .create_item(
            client::CreateItemInput {
                name: "Hello Based".into(),
            },
            (),
        )
        .await?;
    let rows = api.items(client::ItemsInput {}, ()).await?;
    assert!(rows.iter().any(|row| row.id == created.id));
    println!("created: {}", serde_json::to_string(&created)?);
    println!("read: {}", serde_json::to_string(&rows)?);
    Ok(())
}
