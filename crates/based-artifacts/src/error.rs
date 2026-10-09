use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum Error {
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Invalid(String),
    UserOwned(PathBuf),
    Stale(Vec<PathBuf>),
    /// Publication cannot be globally atomic across files. A late failure names exactly
    /// which files were replaced so callers never report a successful/unknown partial set.
    Partial {
        written: Vec<PathBuf>,
        source: Box<Self>,
    },
}

impl Error {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { action, path, source } => write!(f, "{action} {}: {source}", path.display()),
            Self::Invalid(message) => f.write_str(message),
            Self::UserOwned(path) => write!(f, "refusing to overwrite user-owned {}; use --force to replace it explicitly", path.display()),
            Self::Stale(paths) => write!(f, "missing or stale generated artifacts: {}", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
            Self::Partial { written, source } => write!(f, "publication failed after replacing [{}]; remaining outputs were not replaced: {source}", written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Partial { source, .. } => Some(source),
            _ => None,
        }
    }
}
