//! Render reproducible Rust dependencies for the embedded consumer.
use super::options::Dialect;

pub fn manifest(dialect: Dialect) -> String {
    let tls = match dialect {
        Dialect::Sqlite => "",
        _ => ", \"tls-rustls\"",
    };
    format!(
        r#"[package]
name = "based-starter"
version = "0.1.0"
edition = "2021"
rust-version = "{}"

[workspace]

[dependencies]
based-runtime = {{ git = "https://github.com/oscscull/baseddsl.git", rev = "{}", features = ["{}", "id-gen"{}] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#,
        env!("CARGO_PKG_RUST_VERSION"),
        based_version::COMMIT,
        dialect.name(),
        tls
    )
}
