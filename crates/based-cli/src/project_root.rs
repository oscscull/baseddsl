//! Select one manifest directory before dispatching any command.
use crate::error::{io_at, CliError};
use std::path::{Path, PathBuf};

pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::io("reading current directory", e))?;
    let start = explicit.map_or_else(|| cwd.clone(), |path| cwd.join(path));
    if explicit.is_some() {
        return canonical_root(&start);
    }
    let root = start
        .ancestors()
        .find(|dir| dir.join(based_manifest::MANIFEST_NAME).is_file())
        .ok_or_else(|| CliError::usage(format!(
            "no based.toml in {} or its ancestors; run inside a Based project or pass its root directory",
            start.display()
        )))?;
    canonical_root(root)
}

fn canonical_root(root: &Path) -> Result<PathBuf, CliError> {
    if !root.join(based_manifest::MANIFEST_NAME).is_file() {
        return Err(CliError::usage(format!(
            "no based.toml at {}; pass the directory containing the intended manifest",
            root.display()
        )));
    }
    root.canonicalize()
        .map_err(|e| io_at("resolving project root", root, e))
}
