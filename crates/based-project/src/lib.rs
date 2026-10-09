//! Checked compiler projects shared by command-line and optional Cargo generation.
//! No runtime, database, HTTP, output publication, or external tools.
mod check;
mod client;
mod error;
mod parse;
mod presentation;
mod report;
mod sources;

use based_ast::Decl;
use based_manifest::Project;
use based_sema::CheckedSchema;
pub use client::render_client;
pub use error::Error;
pub use report::Report;
use std::path::{Path, PathBuf};

pub type Sources = Vec<(PathBuf, String)>;

pub struct CheckedProject {
    pub project: Project,
    pub schema: CheckedSchema,
    pub declarations: Vec<Decl>,
    pub report: Report,
}

/// Discover, parse and check the manifest's closed schema set before generation.
pub fn load(root: &Path) -> Result<CheckedProject, Error> {
    let project = based_manifest::discover(root).map_err(|diagnostics| {
        Error::Diagnostics(Report {
            sources: Vec::new(),
            diagnostics,
        })
    })?;
    check_project(project)
}

/// Check an already discovered project without changing the discovery contract.
pub fn check_project(project: Project) -> Result<CheckedProject, Error> {
    let sources = sources::read(&project)?;
    let (mut declarations, diagnostics) = parse::declarations(&sources);
    let mut report = Report {
        sources,
        diagnostics,
    };
    report.ensure_clean()?;
    let schema = check::schema(&project, &mut declarations, &mut report)?;
    Ok(CheckedProject {
        project,
        schema,
        declarations,
        report,
    })
}
