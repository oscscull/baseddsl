//! Wire the database and checked schema into the embedded engine.
mod database;
mod demo;
mod lookup;
mod session;

#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}

/// based.toml and schema/, checked and embedded by `build.rs`.
mod schema {
    include!(concat!(env!("OUT_DIR"), "/schema.rs"));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let compiled = based_runtime::Compiled::from_sources(schema::MANIFEST, schema::SOURCES)?;
    let engine = based_runtime::Engine::new(
        compiled,
        database::connect(root)?,
        based_runtime::id::UuidGen,
    );
    demo::run(&engine, session::Session::local_demo()).await?;
    Ok(())
}
