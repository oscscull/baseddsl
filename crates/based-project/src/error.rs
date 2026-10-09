//! Source-located compiler failures usable outside a terminal command.
use crate::{presentation::write_diagnostic, Report};
use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub enum Error {
    Diagnostics(Report),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "reading {}: {source}", path.display()),
            Self::Diagnostics(report) => {
                for diagnostic in &report.diagnostics {
                    write_diagnostic(f, diagnostic, &report.sources)?;
                }
                write!(
                    f,
                    "check failed: {} error(s), {} warning(s) across {} file(s)",
                    report.errors(),
                    report.warnings(),
                    report.sources.len()
                )
            }
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Diagnostics(_) => None,
        }
    }
}
