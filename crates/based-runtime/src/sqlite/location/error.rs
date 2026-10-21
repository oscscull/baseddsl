//! Why a SQLite connection value names no usable database. Messages never echo the
//! value, which may be another dialect's URL carrying credentials.

use std::fmt;

use crate::run::DbError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqliteLocationError {
    /// A URL with a scheme other than `sqlite:`, e.g. a server URL.
    ForeignScheme(String),
    /// `sqlite:` URL query parameters (`?mode=…`), which would override the backend's own
    /// connection settings.
    QueryParameters,
    MissingPath,
    InvalidEscape,
}

impl fmt::Display for SqliteLocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignScheme(scheme) => write!(
                f,
                "sqlite connection must be a file path, :memory:, or a sqlite: URL, not a `{scheme}:` URL (value redacted; write ./name for a relative file whose name contains ':')"
            ),
            Self::QueryParameters => f.write_str(
                "sqlite connection URL query parameters are not supported; pass only the database path (value redacted)",
            ),
            Self::MissingPath => f.write_str("sqlite connection names no database file"),
            Self::InvalidEscape => {
                f.write_str("sqlite connection URL has an invalid %-escape (value redacted)")
            }
        }
    }
}

impl std::error::Error for SqliteLocationError {}

impl From<SqliteLocationError> for DbError {
    fn from(error: SqliteLocationError) -> Self {
        Self::new(error.to_string())
    }
}
