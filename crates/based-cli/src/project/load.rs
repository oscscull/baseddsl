//! Adapt shared compiler results and diagnostics to the CLI exit/rendering contract.
use crate::{
    error::{io_at, CliError},
    render,
};
use based_ast::Decl;
use based_manifest::Project;
use based_sema::CheckedSchema;
use std::path::{Path, PathBuf};

pub type Loaded = (
    Project,
    CheckedSchema,
    Vec<Decl>,
    Vec<(PathBuf, String)>,
    usize,
);

pub fn load_checked(root: &Path) -> Result<Loaded, CliError> {
    let checked = based_project::load(root).map_err(|error| report_error(root, error))?;
    render::render(&checked.report.diagnostics, &checked.report.sources);
    let warnings = checked.report.warnings();
    Ok((
        checked.project,
        checked.schema,
        checked.declarations,
        checked.report.sources,
        warnings,
    ))
}

fn report_error(root: &Path, error: based_project::Error) -> CliError {
    match error {
        based_project::Error::Io { path, source } => io_at("reading", &path, source),
        based_project::Error::Diagnostics(report) => {
            render::render(&report.diagnostics, &report.sources);
            let message = match report.sources.is_empty() {
                true => format!("could not load project at {} (see above)", root.display()),
                false => format!(
                    "check failed: {} error(s), {} warning(s) across {} file(s)",
                    report.errors(),
                    report.warnings(),
                    report.sources.len()
                ),
            };
            CliError::summary(true, message)
        }
    }
}
