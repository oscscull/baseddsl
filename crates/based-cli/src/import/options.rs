//! One-shot import arguments; connection secrets use the existing local configuration.
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct Options {
    /// Explicit project root; otherwise use the nearest ancestor based.toml.
    pub root: Option<PathBuf>,
    /// One database URL or SQLite file; falls back to project/env connection configuration.
    #[arg(long)]
    pub database_url: Option<String>,
    /// Exact physical namespace.table identity; repeat for every selected table.
    /// SQLite's namespace is main. No wildcards or implicit referenced-table selection.
    #[arg(long = "table", required = true)]
    pub tables: Vec<String>,
    /// Destination directory under the configured schema root, relative to the project.
    /// Defaults to the configured root; existing model files are never replaced.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Print a structured report with native catalog facts, losses and compiler diagnostics.
    #[arg(long)]
    pub json: bool,
}
