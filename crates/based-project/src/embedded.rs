//! Check manifest text and named sources that were compiled into a binary.
use crate::{check_sources, CheckedProject, Error};
use based_diagnostics::Diagnostic;
use based_manifest::{DiscoveredFile, Project};
use std::path::PathBuf;

/// Check `based.toml` text plus `(name, text)` sources; diagnostics cite each `name`.
pub fn check_embedded(manifest: &str, files: &[(&str, &str)]) -> Result<CheckedProject, Error> {
    let manifest = based_manifest::parse(manifest).map_err(Error::manifest)?;
    if files.is_empty() {
        return Err(Error::manifest(vec![Diagnostic::error(
            "E0012",
            "no embedded `.bsl` sources",
        )]));
    }
    let sources: crate::Sources = files
        .iter()
        .map(|(name, text)| (PathBuf::from(name), (*text).to_owned()))
        .collect();
    let files = sources
        .iter()
        .map(|(path, _)| DiscoveredFile { path: path.clone() })
        .collect();
    check_sources(Project { manifest, files }, sources)
}
