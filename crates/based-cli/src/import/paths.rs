//! Resolve the configured model destination and protect reviewed/source paths.
use crate::error::{io_at, CliError};
use based_manifest::Manifest;
use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

pub(super) fn output(
    root: &Path,
    manifest: &Manifest,
    requested: Option<&Path>,
) -> Result<PathBuf, CliError> {
    let schema = normalize(&root.join(manifest.root.as_deref().unwrap_or(".")));
    let output = requested.map_or_else(|| schema.clone(), |path| normalize(&root.join(path)));
    let text = output
        .to_str()
        .ok_or_else(|| CliError::usage("import output must be a UTF-8 path for its report"))?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(CliError::usage(
            "import output must be a nonempty path without control characters",
        ));
    }
    if !output.starts_with(&schema)
        || ["migrations", ".git", ".agents", ".codex"]
            .iter()
            .any(|protected| output.starts_with(root.join(protected)))
    {
        return Err(CliError::usage("import output must be within the configured schema root and outside migration/configuration history"));
    }
    require_directories(&output)?;
    Ok(output)
}

fn normalize(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut path, component| {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    path.pop();
                }
                _ => path.push(component),
            }
            path
        })
}

pub(super) fn require_directories(output: &Path) -> Result<(), CliError> {
    for directory in output.ancestors() {
        match fs::symlink_metadata(directory) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(io_at("inspecting import output", directory, error)),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(CliError::usage(
                    "import output must traverse ordinary directories, never files or symlinks",
                ))
            }
        }
    }
    Ok(())
}
