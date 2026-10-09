use crate::{command, consumer};
use anyhow::Result;
use std::path::Path;

pub fn verify(based: &Path) -> Result<()> {
    let scratch = tempfile::tempdir()?;
    let environment = crate::source::mirror(scratch.path())?;
    let commit = command::git(&["rev-parse", "HEAD"])?;
    consumer::verify_with(
        "sqlite",
        &based.canonicalize()?,
        Some(&commit),
        &environment,
    )
}
