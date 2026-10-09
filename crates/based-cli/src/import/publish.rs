//! Stage the complete model set and create new files exclusively; never regenerate hand edits.
use crate::error::{io_at, CliError};
use based_project::Sources;
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

pub(super) struct Failure {
    pub written: Vec<PathBuf>,
    pub error: CliError,
}

pub(super) fn preflight(directory: &Path, files: &Sources) -> Result<(), CliError> {
    super::paths::require_directories(directory)?;
    for (name, _) in files {
        let target = directory.join(name);
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == ErrorKind::NotFound => {},
            Err(error) => return Err(io_at("inspecting import model", &target, error)),
            Ok(_) => return Err(CliError::usage(format!("import never replaces an existing entry: {}; choose a fresh directory under the schema root and review/merge manually", target.display()))),
        }
    }
    Ok(())
}

pub(super) fn write(directory: &Path, files: &Sources) -> Result<Vec<PathBuf>, Failure> {
    let staged = stage(directory, files).map_err(|error| Failure {
        written: Vec::new(),
        error,
    })?;
    let mut written = Vec::new();
    for (path, file) in staged {
        if let Err(error) = file.persist_noclobber(&path) {
            return Err(Failure {
                written,
                error: io_at(
                    "creating new import model; existing entries are never replaced",
                    &path,
                    error.error,
                ),
            });
        }
        written.push(path);
    }
    Ok(written)
}

fn stage(
    directory: &Path,
    files: &Sources,
) -> Result<Vec<(PathBuf, tempfile::NamedTempFile)>, CliError> {
    preflight(directory, files)?;
    fs::create_dir_all(directory)
        .map_err(|error| io_at("creating import directory", directory, error))?;
    files
        .iter()
        .map(|(path, source)| {
            let target = directory.join(path);
            let mut file = tempfile::NamedTempFile::new_in(directory)
                .map_err(|error| io_at("staging import model", &target, error))?;
            file.write_all(source.as_bytes())
                .map_err(|error| io_at("staging import model", &target, error))?;
            file.as_file()
                .sync_all()
                .map_err(|error| io_at("syncing import model", &target, error))?;
            Ok((target, file))
        })
        .collect()
}
