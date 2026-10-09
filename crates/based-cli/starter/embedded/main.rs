//! Wire the database and checked schema into the embedded engine.
mod database;
mod demo;
mod lookup;
mod session;

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
    demo::run(&engine, session::Session::local_demo()).await?;
    Ok(())
}
