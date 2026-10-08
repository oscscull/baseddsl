//! Open the configured local SQLite file; migration application is separate.
pub fn connect(
    root: &std::path::Path,
) -> Result<based_runtime::SqliteBackend, based_runtime::DbError> {
    let configured = std::env::var("DATABASE_URL").unwrap_or_else(|_| "local.db".into());
    let path = std::path::Path::new(&configured);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    based_runtime::SqliteBackend::open(&absolute.to_string_lossy())
}
