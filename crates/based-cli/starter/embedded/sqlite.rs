//! Open the configured local SQLite database; migration application is separate.
pub fn connect(
    root: &std::path::Path,
) -> Result<based_runtime::SqliteBackend, based_runtime::DbError> {
    let configured = std::env::var("DATABASE_URL").unwrap_or_else(|_| "local.db".into());
    let location = based_runtime::SqliteLocation::parse(&configured)?.anchored(root);
    based_runtime::SqliteBackend::open_location(&location)
}
