//! Assemble a complete owned artifact before publication begins.
use super::{
    command, output, path,
    render::{self, Kind},
};
use crate::{error::CliError, project::Loaded};
use based_artifacts::Artifact;
use based_manifest::ClientMode;
use std::path::Path;

pub(super) fn bytes(
    kind: Kind,
    loaded: &Loaded,
    mode: ClientMode,
    destination: Option<&Path>,
) -> Result<Vec<u8>, CliError> {
    let content = render::render(kind, loaded, mode)?;
    let command = command::regeneration(kind, destination, mode == ClientMode::Embedded);
    kind.format()
        .annotate(&content, &command)
        .map_err(output::error)
}

pub(super) fn file(
    kind: Kind,
    root: &Path,
    destination: &Path,
    loaded: &Loaded,
    mode: ClientMode,
) -> Result<Artifact, CliError> {
    Ok(Artifact {
        path: path::destination(root, destination)?,
        bytes: bytes(kind, loaded, mode, Some(destination))?,
        format: kind.format(),
    })
}
