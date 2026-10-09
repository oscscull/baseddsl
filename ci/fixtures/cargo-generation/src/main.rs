#[allow(dead_code)]
mod client {
    include!(concat!(env!("OUT_DIR"), "/client.rs"));
}

#[tokio::main]
async fn main() {
    // Runtime loading stays explicit. Build-time client generation supplies no engine assets.
    let compiled = based_runtime::Compiled::load(std::path::Path::new(".")).unwrap();
    let row = serde_json::json!({ "id": 1, "name": "Ada" }).as_object().unwrap().clone();
    let db = based_runtime::MockDb::new(vec![vec![row]]);
    let engine = based_runtime::Engine::new(compiled, db, based_runtime::SeqIdGen::default());
    let api = client::embedded(&engine);
    let rows = api.items(client::ItemsInput {}, ()).await.unwrap();
    assert_eq!(rows[0].name, "Ada");
}
