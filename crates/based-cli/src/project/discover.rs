//! Discover a project without checking an in-progress schema.
use crate::{error::CliError, render};
use based_manifest::Project;
use std::path::Path;

/// Discover the project (manifest + files) without running the full front end — apply/status
/// only need the manifest dialect, and must work against an in-progress schema.
pub fn discover_project(root: &Path) -> Result<Project, CliError> {
    match based_manifest::discover(root) {
        Ok(p) => Ok(p),
        Err(diags) => {
            render::render(&diags, &[]);
            Err(CliError::summary(
                true,
                format!("could not load project at {} (see above)", root.display()),
            ))
        }
    }
}
