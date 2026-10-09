//! Publish a fully prepared starter without replacing any existing entry.
use crate::error::{io_at, CliError};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub fn require_empty(root: &Path) -> Result<(), CliError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_at("inspecting", root, error)),
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.is_symlink() {
                return Err(CliError::usage(
                    "init requires an empty directory, not a file or symlink",
                ));
            }
            if fs::read_dir(root)
                .map_err(|e| io_at("reading", root, e))?
                .next()
                .is_some()
            {
                return Err(CliError::usage("init requires an empty directory; existing Rust projects are not modified. Initialize a new sibling directory, then integrate the generated client explicitly."));
            }
            Ok(())
        }
    }
}

pub fn publish(prepared: &Path, root: &Path) -> Result<(), CliError> {
    require_empty(root)?;
    copy_new(prepared, root)
}

fn copy_new(source: &Path, destination: &Path) -> Result<(), CliError> {
    fs::create_dir_all(destination).map_err(|e| io_at("creating", destination, e))?;
    for entry in fs::read_dir(source).map_err(|e| io_at("reading", source, e))? {
        let entry = entry.map_err(|e| io_at("reading", source, e))?;
        let output = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|e| io_at("inspecting", &entry.path(), e))?
            .is_dir()
        {
            fs::create_dir(&output).map_err(|e| io_at("creating new directory", &output, e))?;
            copy_new(&entry.path(), &output)?;
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(|e| io_at("reading", &entry.path(), e))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| {
                io_at(
                    "creating new starter file; existing entries are never replaced",
                    &output,
                    e,
                )
            })?;
        file.write_all(&bytes)
            .map_err(|e| io_at("writing", &output, e))?;
    }
    Ok(())
}
