//! The embedded tutorial's commands and schema files: executed by the gate and required verbatim in the book.
use anyhow::Result;
use std::path::{Path, PathBuf};

pub const INIT: &[&str] = &["based", "init", "items", "--mode", "embedded"];
pub const MIGRATE: &[&str] = &["based", "migrate", "apply", "--database-url", "local.db"];
pub const RUN: &[&str] = &["cargo", "run"];
pub const EVOLVE: &[&[&str]] = &[
    &["based", "check"],
    &["based", "migrate", "gen", ".", "rename_item_title"],
    &["based", "migrate", "verify"],
    &["based", "migrate", "render", ".", "--number", "2"],
];
pub const RESET: &[&str] = &[
    "rm",
    "-f",
    "local.db",
    "local.db-journal",
    "local.db-wal",
    "local.db-shm",
];

const ALL: &[&[&str]] = &[
    INIT, MIGRATE, RUN, EVOLVE[0], EVOLVE[1], EVOLVE[2], EVOLVE[3], RESET,
];

/// The starter schema, as `based init` writes it.
pub fn starter_schema() -> PathBuf {
    crate::command::root().join("crates/based-cli/starter/schema/item.bsl")
}

/// The same schema after the tutorial's data-preserving rename.
pub fn renamed_schema() -> PathBuf {
    crate::command::root().join("crates/based-cli/starter/tutorial/item.bsl")
}

/// Fail when the book no longer shows a command the gate runs or a schema it applies.
pub fn require_documented(book: &Path) -> Result<()> {
    crate::book::require_commands(book, ALL)?;
    crate::book::require_files(book, &[&starter_schema(), &renamed_schema()])
}
