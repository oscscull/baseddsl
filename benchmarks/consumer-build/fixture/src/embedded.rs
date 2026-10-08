#[cfg(feature = "cargo-generation")]
#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}
#[cfg(not(feature = "cargo-generation"))]
#[allow(dead_code)]
mod client {
    include!("../generated/client.rs");
}

pub async fn rows(pool: based_runtime::sqlx::SqlitePool) -> serde_json::Value {
    let schema = based_runtime::Compiled::load(std::path::Path::new(".")).unwrap();
    let backend = based_runtime::SqliteBackend::from_pool(pool);
    let engine = based_runtime::Engine::new(schema, backend, based_runtime::id::UuidGen);
    let rows = client::embedded(&engine)
        .items(client::ItemsInput {}, ())
        .await
        .unwrap();
    serde_json::to_value(rows).unwrap()
}
