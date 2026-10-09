//! Render with shared client options and publish only the fixed Cargo output artifact.
use crate::Error;
use based_artifacts::{Artifact, Format, Policy};
use based_project::CheckedProject;
use std::path::Path;

pub(super) fn client(checked: &CheckedProject) -> Result<Vec<u8>, Error> {
    let text = based_project::render_client(
        &checked.project,
        &checked.schema,
        &checked.declarations,
        checked.project.manifest.generate.client_mode,
    );
    Ok(Format::Rust.annotate(&text, "cargo build")?)
}

pub(super) fn publish(path: &Path, bytes: Vec<u8>) -> Result<bool, Error> {
    let artifact = Artifact {
        path: path.to_path_buf(),
        bytes,
        format: Format::Rust,
    };
    let report = based_artifacts::publish(
        &[artifact],
        Policy::Write {
            overwrite_user_owned: false,
        },
    )?;
    Ok(!report.written.is_empty())
}
