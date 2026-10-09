//! Write complete replacement bytes into a temporary sibling before touching the target.
use crate::{Artifact, Error};
use std::{fs, io::Write, path::Path};
use tempfile::NamedTempFile;

pub(crate) struct Staged<'a> {
    pub artifact: &'a Artifact,
    pub file: NamedTempFile,
}

pub(crate) fn stage(artifact: &Artifact) -> Result<Staged<'_>, Error> {
    let parent = artifact.path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| Error::io("creating directory", parent, error))?;
    let mut file = NamedTempFile::new_in(parent)
        .map_err(|error| Error::io("staging", &artifact.path, error))?;
    file.write_all(&artifact.bytes)
        .map_err(|error| Error::io("staging", &artifact.path, error))?;
    if let Ok(metadata) = fs::metadata(&artifact.path) {
        file.as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| Error::io("preserving permissions", &artifact.path, error))?;
    }
    file.as_file()
        .sync_all()
        .map_err(|error| Error::io("syncing", &artifact.path, error))?;
    Ok(Staged { artifact, file })
}
