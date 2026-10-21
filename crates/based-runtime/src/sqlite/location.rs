//! Where a SQLite database lives, read from a bare path or a `sqlite:` URL.

mod error;
mod percent_decode;
mod scheme;
#[cfg(test)]
mod tests;

pub use error::SqliteLocationError;

use std::path::{Path, PathBuf};

/// A SQLite database target: `:memory:` or a file path. Parsed from a bare path,
/// `:memory:`, or the `sqlite:` / `sqlite://` / `sqlite:///abs` URL forms; any other
/// scheme is rejected so a server URL is never mistaken for a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqliteLocation {
    Memory,
    File(PathBuf),
}

impl SqliteLocation {
    pub fn parse(value: &str) -> Result<Self, SqliteLocationError> {
        match scheme::of(value) {
            None => Self::from_path(value),
            Some(name) if name.eq_ignore_ascii_case("sqlite") => {
                Self::from_url_rest(&value[name.len() + 1..])
            }
            Some(name) => Err(SqliteLocationError::ForeignScheme(name.to_owned())),
        }
    }

    /// Resolve a relative file path against `root`; memory and absolute paths are unchanged.
    #[must_use]
    pub fn anchored(self, root: &Path) -> Self {
        match self {
            Self::File(path) if path.is_relative() => Self::File(root.join(path)),
            other => other,
        }
    }

    /// The bare-path spelling, which [`parse`](Self::parse) reads back unchanged for
    /// memory and absolute paths.
    pub fn to_path_string(&self) -> String {
        match self {
            Self::Memory => ":memory:".to_owned(),
            Self::File(path) => path.to_string_lossy().into_owned(),
        }
    }

    fn from_path(path: &str) -> Result<Self, SqliteLocationError> {
        match path {
            "" => Err(SqliteLocationError::MissingPath),
            ":memory:" => Ok(Self::Memory),
            _ => Ok(Self::File(PathBuf::from(path))),
        }
    }

    fn from_url_rest(rest: &str) -> Result<Self, SqliteLocationError> {
        let path = rest.strip_prefix("//").unwrap_or(rest);
        if path.contains('?') {
            return Err(SqliteLocationError::QueryParameters);
        }
        Self::from_path(&percent_decode::decode(path)?)
    }
}
