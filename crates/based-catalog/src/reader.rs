//! The metadata-only asynchronous adapter seam and deliberately redacted operational errors.
use super::{Discovery, Selection};
use std::{fmt, future::Future};

pub trait CatalogReader {
    /// Use a read-only connection/transaction and metadata queries only.
    /// No row sampling, database mutation, dependency expansion, or BSL writing.
    fn discover(
        &mut self,
        selection: &Selection,
    ) -> impl Future<Output = Result<Discovery, CatalogReadError>> + Send;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogReadError {
    Connection,
    Metadata,
    InconsistentSnapshot,
}

impl fmt::Display for CatalogReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Connection => "Cannot connect for catalog discovery; verify the URL, credentials, and read-only metadata privileges",
            Self::Metadata => "Cannot read complete catalog metadata; verify selected names and metadata privileges",
            Self::InconsistentSnapshot => "Catalog changed during discovery; retry against a stable schema",
        };
        f.write_str(message)
    }
}

impl std::error::Error for CatalogReadError {}
