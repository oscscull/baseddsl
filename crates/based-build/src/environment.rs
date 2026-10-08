//! Resolve Cargo's package and generated-output directories; destinations are fixed.
use crate::Error;
use std::path::PathBuf;

pub(super) struct Environment {
    pub root: PathBuf,
    pub out: PathBuf,
}
impl Environment {
    pub(super) fn cargo() -> Result<Self, Error> {
        Ok(Self {
            root: variable("CARGO_MANIFEST_DIR")?,
            out: variable("OUT_DIR")?,
        })
    }
}
fn variable(name: &'static str) -> Result<PathBuf, Error> {
    let value = std::env::var_os(name).ok_or(Error::Environment(name))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(Error::Environment(name));
    }
    Ok(path)
}
