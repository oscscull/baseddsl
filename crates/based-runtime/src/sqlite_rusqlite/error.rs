use crate::run::{DbError, DbErrorKind};

/// Map a rusqlite error to a [`DbError`], worded and classified as the sqlx driver does:
/// `SQLITE_BUSY` (5) / `SQLITE_LOCKED` (6) after the busy timeout is a retryable deadlock.
pub(super) fn map_err(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(code, message) => {
            // Extended result codes carry the primary code in the low byte.
            let kind = match code.extended_code & 0xff {
                5 | 6 => DbErrorKind::Deadlock,
                _ => DbErrorKind::Other,
            };
            let message = message.unwrap_or_else(|| code.to_string());
            DbError::of(
                kind,
                format!(
                    "error returned from database: (code: {}) {message}",
                    code.extended_code
                ),
            )
        }
        other => DbError::new(other.to_string()),
    }
}
