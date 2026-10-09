//! Resolve artifact paths and protect reviewed migration history from generic generation.
use crate::error::{io_at, CliError};
use std::{
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

pub(super) fn destination(root: &Path, out: &Path) -> Result<PathBuf, CliError> {
    let text = out.to_str().ok_or_else(|| {
        CliError::usage("artifact destination must be UTF-8 for its regeneration command")
    })?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(CliError::usage(
            "artifact destination must be a nonempty path without control characters",
        ));
    }
    let absolute = normalize(&root.join(out));
    // Resolve parent aliases while preserving the final entry: publication rejects
    // symlinks rather than following them and replacing their targets.
    let parent = absolute
        .parent()
        .ok_or_else(|| CliError::usage("artifact destination must name a file"))?;
    let name = absolute
        .file_name()
        .ok_or_else(|| CliError::usage("artifact destination must name a file"))?;
    let resolved = resolve_existing(parent)?.join(name);
    let migrations = resolve_existing(&root.join("migrations"))?;
    if resolved.starts_with(migrations) {
        return Err(CliError::usage(
            "generic generation cannot write migration history; use based migrate gen",
        ));
    }
    Ok(resolved)
}

fn normalize(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut result, component| {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    result.pop();
                }
                _ => result.push(component),
            }
            result
        })
}

fn resolve_existing(path: &Path) -> Result<PathBuf, CliError> {
    match std::fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let Some(parent) = path.parent() else {
                return Err(io_at("resolving", path, error));
            };
            let Some(name) = path.file_name() else {
                return Err(io_at("resolving", path, error));
            };
            Ok(resolve_existing(parent)?.join(name))
        }
        Err(error) => Err(io_at("resolving", path, error)),
    }
}
