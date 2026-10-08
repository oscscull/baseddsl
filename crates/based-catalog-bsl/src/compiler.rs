//! Format and check supplied model sources through the ordinary project pipeline.
use based_manifest::{Manifest, Project};
use based_project::{CheckedProject, Report, Sources};
use std::path::Path;

pub(crate) fn check(files: &mut Sources, manifest: &Manifest) -> Result<CheckedProject, Report> {
    for (_, source) in files.iter_mut() {
        match based_fmt::format_source(source) {
            Ok(formatted) => *source = formatted,
            Err(diagnostics) => {
                return Err(based_project::Report {
                    sources: files.clone(),
                    diagnostics,
                });
            }
        }
    }
    let root = Path::new(manifest.root.as_deref().unwrap_or("."));
    let sources: Sources = files
        .iter()
        .map(|(path, text)| (root.join(path), text.clone()))
        .collect();
    let project = Project {
        manifest: manifest.clone(),
        files: sources
            .iter()
            .map(|(path, _)| based_manifest::DiscoveredFile { path: path.clone() })
            .collect(),
    };
    let checked = match based_project::check_sources(project, sources) {
        Ok(checked) => checked,
        Err(based_project::Error::Diagnostics(compiler)) => return Err(compiler),
        Err(based_project::Error::Io { .. }) => {
            unreachable!("supplied-source checking performs no file I/O")
        }
    };
    Ok(checked)
}
