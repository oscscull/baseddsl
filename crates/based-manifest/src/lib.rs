//! based-manifest — project manifest + schema discovery.
//!
//! Resolves `based.toml` and globs `**/*.bsl` under the schema root into the
//! closed file set the rest of the compiler consumes. Layout is free; directory
//! structure is the user's choice, not enforced.

use based_diagnostics::Diagnostic;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

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

/// The manifest file name at a project root.
pub const MANIFEST_NAME: &str = "based.toml";

/// Load the manifest at `root/based.toml` and glob `**/*.bsl` under the schema
/// root into the closed file set. Directory layout is not constrained ; files
/// are returned in a stable, path-sorted order so diagnostics are deterministic.
pub fn discover(root: &Path) -> Result<Project, Vec<Diagnostic>> {
    let manifest_path = root.join(MANIFEST_NAME);
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        vec![Diagnostic::error(
            "E0010",
            format!("cannot read {}: {e}", manifest_path.display()),
        )]
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| {
        vec![Diagnostic::error(
            "E0011",
            format!("invalid {MANIFEST_NAME}: {e}"),
        )]
    })?;

    // Schema root: the manifest's `root` (relative to the manifest dir), else the
    // manifest dir itself.
    let schema_root = match &manifest.root {
        Some(r) => root.join(r),
        None => root.to_path_buf(),
    };

    let mut files: Vec<DiscoveredFile> = WalkDir::new(&schema_root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "bsl"))
        .map(|e| DiscoveredFile {
            path: e.into_path(),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    if files.is_empty() {
        return Err(vec![Diagnostic::error(
            "E0012",
            format!("no `.bsl` files found under {}", schema_root.display()),
        )]);
    }

    Ok(Project { manifest, files })
}
