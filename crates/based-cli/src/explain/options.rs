//! `based explain` arguments.
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct ExplainOptions {
    /// Explicit project root; otherwise find the nearest ancestor based.toml.
    pub root: Option<PathBuf>,
    /// The query or mutation to explain. Omitted, every callable is printed.
    pub callable: Option<String>,
    /// Print every callable.
    #[arg(long, conflicts_with = "callable")]
    pub all: bool,
    /// Override the target dialect (`mariadb`/`mysql`/`sqlite`/`postgres`). Defaults to
    /// the manifest dialect.
    #[arg(long)]
    pub dialect: Option<String>,
}

impl ExplainOptions {
    /// A lone positional that is not a directory names the callable, not the root.
    pub fn root_and_callable(self) -> (Option<PathBuf>, Option<String>) {
        match (self.root, self.callable, self.all) {
            (Some(root), None, false) if !root.is_dir() => {
                (None, Some(root.to_string_lossy().into_owned()))
            }
            (root, callable, _) => (root, callable),
        }
    }
}
