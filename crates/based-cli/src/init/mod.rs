//! Prepare and verify a complete starter before creating destination files.
mod cargo;
mod config;
mod files;
mod instructions;
mod manifest;
mod migration;
pub mod options;
mod revision;
mod templates;

use crate::error::{io_at, CliError};
use options::Options;

pub fn execute(options: Options) -> Result<(), CliError> {
    files::require_empty(&options.root)?;
    if matches!(options.mode, options::Mode::Embedded) {
        revision::require_known()?;
    }
    let stage = tempfile::tempdir().map_err(|e| CliError::io("preparing starter directory", e))?;
    for (relative, content) in templates::files(&options) {
        let path = stage.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_at("creating", parent, e))?;
        }
        std::fs::write(&path, content).map_err(|e| io_at("preparing", &path, e))?;
    }
    based_artifacts::publish(
        &crate::gen::prepared_all(stage.path())?,
        based_artifacts::Policy::Write {
            overwrite_user_owned: false,
        },
    )
    .map_err(|e| CliError::caused_by("preparing generated starter artifacts", e))?;
    migration::prepare(stage.path())?;
    files::publish(stage.path(), &options.root)?;
    instructions::print(&options);
    Ok(())
}
