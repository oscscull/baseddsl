mod http;
mod service;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = service::Service::start()?;
    let api = http::Api::new(service.url())?;
    let parent = api.call(
        "m/create_item",
        serde_json::json!({"name": "Parent", "parent": null}),
    )?;
    let created = api.call(
        "m/create_item",
        serde_json::json!({"name": "Hello Based", "parent": parent["id"]}),
    )?;
    let rows = api.call("q/items", serde_json::json!({}))?;
    println!("created: {created}");
    println!("read: {rows}");
    Ok(())
}
