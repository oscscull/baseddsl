//! Check imported models together with all existing configured BSL before any publication.
use crate::error::{io_at, CliError};
use based_manifest::{DiscoveredFile, Project};
use based_project::{Report, Sources};
use std::path::Path;

pub(super) enum Error {
    Io(CliError),
    Compiler(Report),
}

pub(super) fn combined(
    mut project: Project,
    directory: &Path,
    models: &Sources,
) -> Result<Report, Error> {
    let mut sources = project
        .files
        .iter()
        .map(|file| {
            std::fs::read_to_string(&file.path)
                .map(|source| (file.path.clone(), source))
                .map_err(|error| Error::Io(io_at("reading existing model", &file.path, error)))
        })
        .collect::<Result<Sources, Error>>()?;
    for (path, source) in models {
        let path = directory.join(path);
        project.files.push(DiscoveredFile { path: path.clone() });
        sources.push((path, source.clone()));
    }
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    project
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    match based_project::check_sources(project, sources) {
        Ok(checked) => Ok(checked.report),
        Err(based_project::Error::Diagnostics(report)) => Err(Error::Compiler(report)),
        Err(based_project::Error::Io { path, source }) => {
            Err(Error::Io(io_at("checking imported models", &path, source)))
        }
    }
}
