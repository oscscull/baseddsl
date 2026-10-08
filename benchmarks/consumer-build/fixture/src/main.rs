mod database;
#[cfg(feature = "direct")]
mod direct;
#[cfg(feature = "embedded")]
mod embedded;

#[tokio::main]
async fn main() {
    let pool = database::seed().await;
    #[cfg(feature = "direct")]
    let rows = direct::rows(pool).await;
    #[cfg(feature = "embedded")]
    let rows = embedded::rows(pool).await;
    assert_eq!(
        rows,
        serde_json::json!([{ "id": "00000000-0000-4000-8000-000000000001", "name": "Ada" }])
    );
    rust_edit_marker();
}

fn rust_edit_marker() {
    std::hint::black_box(0);
}
