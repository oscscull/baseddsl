//! The standalone tutorial's commands: executed by the gate and required verbatim in the book.
use anyhow::{ensure, Result};
use std::path::Path;

pub const INIT: &[&str] = &["based", "init", "items", "--mode", "standalone"];
pub const MIGRATE: &[&str] = &["based", "migrate", "apply", "--database-url", "local.db"];
pub const SERVE_FIRST: &[&str] = &[
    "based",
    "serve",
    "--listen",
    "127.0.0.1:9090",
    "--database-url",
    "local.db",
    "--init-idempotency-table",
];
pub const SERVE: &[&str] = &[
    "based",
    "serve",
    "--listen",
    "127.0.0.1:9090",
    "--database-url",
    "local.db",
];
pub const EDGE: &[&str] = &["node", "edge/edge.mjs"];
pub const CREATE: &[&str] = &[
    "curl",
    "-s",
    "localhost:8080/m/create_item",
    "-H",
    "Authorization: Bearer alice-local-token",
    "-H",
    "Idempotency-Key: first-item",
    "-d",
    r#"{"name":"First","parent":null}"#,
];
pub const REUSE: &[&str] = &[
    "curl",
    "-s",
    "localhost:8080/m/create_item",
    "-H",
    "Authorization: Bearer alice-local-token",
    "-H",
    "Idempotency-Key: first-item",
    "-d",
    r#"{"name":"Changed","parent":null}"#,
];
pub const LIST: &[&str] = &[
    "curl",
    "-s",
    "localhost:8080/q/items",
    "-H",
    "Authorization: Bearer alice-local-token",
    "-d",
    "{}",
];
pub const FORGED: &[&str] = &[
    "curl",
    "-s",
    "localhost:8080/q/items",
    "-H",
    "Authorization: Bearer bob-local-token",
    "-H",
    r#"X-Based-Context: {"owner":"00000000-0000-4000-8000-00000000000a"}"#,
    "-d",
    "{}",
];
pub const ANONYMOUS: &[&str] = &["curl", "-s", "localhost:8080/q/items", "-d", "{}"];
pub const EVOLVE: &[&[&str]] = &[
    &["based", "gen", "all"],
    &["based", "migrate", "gen"],
    &["based", "migrate", "verify"],
];

const ALL: &[&[&str]] = &[
    INIT,
    MIGRATE,
    SERVE_FIRST,
    SERVE,
    EDGE,
    CREATE,
    REUSE,
    LIST,
    FORGED,
    ANONYMOUS,
    EVOLVE[0],
    EVOLVE[1],
    EVOLVE[2],
];

/// Fail when the book no longer shows a command the gate runs.
pub fn require_documented(book: &Path) -> Result<()> {
    let text = std::fs::read_to_string(book)?.replace("\\\n", " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for args in ALL {
        let line = shell(args);
        ensure!(
            text.contains(&line),
            "{} no longer shows: {line}",
            book.display()
        );
    }
    Ok(())
}

fn shell(args: &[&str]) -> String {
    args.iter()
        .map(|arg| {
            let plain = arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c));
            if plain {
                arg.to_string()
            } else {
                format!("'{arg}'")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
