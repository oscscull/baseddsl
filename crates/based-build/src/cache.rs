//! Validate cached compiler inputs against the current generated client bytes.
use crate::Error;
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(super) struct Cache {
    path: PathBuf,
}
impl Cache {
    pub(super) fn new(out: &Path) -> Self {
        Self {
            path: out.join("based-client.inputs"),
        }
    }
    pub(super) fn matches(&self, inputs: &str, output: &Path) -> Result<bool, Error> {
        match std::fs::symlink_metadata(output) {
            Ok(metadata) if !metadata.file_type().is_file() => return Ok(false),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let Some(record) = read(&self.path)? else {
            return Ok(false);
        };
        let Some(bytes) = read(output)? else {
            return Ok(false);
        };
        Ok(record == signature(inputs, &bytes).as_bytes())
    }
    pub(super) fn record(&self, inputs: &str, output: &Path) -> Result<(), Error> {
        let bytes = std::fs::read(output)?;
        let parent = self.path.parent().expect("cache has output directory");
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        staged.write_all(signature(inputs, &bytes).as_bytes())?;
        staged.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }
}
fn signature(inputs: &str, output: &[u8]) -> String {
    format!("{inputs}\n{:x}\n", Sha256::digest(output))
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
