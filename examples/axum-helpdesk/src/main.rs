//! Boot the helpdesk: load `.env`, wire the engine over the app's own pool, serve.

use axum_helpdesk::{routes, App};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL (see .env)");
    let listen = std::env::var("LISTEN").unwrap_or_else(|_| "127.0.0.1:8000".to_string());

    let app = App::connect(&url).await;
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| panic!("bind {listen}: {e}"));
    println!("helpdesk listening on http://{listen}");
    axum::serve(listener, routes::router(app))
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await
        .expect("serve");
}
