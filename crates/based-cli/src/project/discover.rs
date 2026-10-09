//! Discover a project without checking an in-progress schema.
use crate::{error::CliError, render};
use based_manifest::Project;
use std::path::Path;

/// Discover the project (manifest + files) without running the full front end — apply/status
/// only need the manifest dialect, and must work against an in-progress schema.
pub fn discover_project(root: &Path) -> Result<Project, CliError> {
    result(based_manifest::discover(root), root)
}

/// Keep manifest/filesystem validation while allowing a caller to stage its first models.
pub fn discover_allow_empty_project(root: &Path) -> Result<Project, CliError> {
    result(based_manifest::discover_allow_empty(root), root)
}

fn result(
    discovery: Result<Project, Vec<based_diagnostics::Diagnostic>>,
    root: &Path,
) -> Result<Project, CliError> {
    match discovery {
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
