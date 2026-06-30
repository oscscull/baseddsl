//! based-manifest — project manifest + schema discovery (decisions.md D5).
//!
//! Resolves `based.toml` and globs `**/*.bsl` under the schema root into the
//! closed file set the rest of the compiler consumes. Layout is free; directory
//! structure is the user's choice, not enforced.

use based_diagnostics::Diagnostic;
use serde::Deserialize;
use std::path::PathBuf;

/// Parsed `based.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_dialect")]
    pub dialect: String,
    #[serde(default = "default_client")]
    pub client: String,
    /// Schema root relative to the manifest; defaults to manifest dir.
    #[serde(default)]
    pub root: Option<String>,
}

fn default_dialect() -> String {
    "mariadb".to_string()
}
fn default_client() -> String {
    "rust".to_string()
}

/// One `.bsl` source file located by discovery.
#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    pub path: PathBuf,
}

/// The closed set of schema files plus the manifest that scoped them.
#[derive(Debug, Clone)]
pub struct Project {
    pub manifest: Manifest,
    pub files: Vec<DiscoveredFile>,
}

/// Load the manifest at `root/based.toml` and glob `**/*.bsl` under the schema root.
pub fn discover(_root: &std::path::Path) -> Result<Project, Vec<Diagnostic>> {
    // TODO(parser-milestone): read based.toml, walk the schema root for `*.bsl`,
    // return the closed file set. Directory layout is not constrained.
    todo!("manifest discovery — parser milestone")
}
