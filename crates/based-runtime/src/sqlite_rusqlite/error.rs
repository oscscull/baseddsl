use crate::run::{DbError, DbErrorKind};

/// Map a rusqlite error to a [`DbError`], worded and classified as the sqlx driver does:
/// `SQLITE_BUSY` (5) / `SQLITE_LOCKED` (6) after the busy timeout is a retryable deadlock,
/// and a constraint violation is its integrity kind.
pub(super) fn map_err(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(code, message) => {
            let kind = kind_of(code.extended_code);
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

/// Classify an SQLite extended result code.
fn kind_of(extended_code: i32) -> DbErrorKind {
    match extended_code {
        rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY => {
            DbErrorKind::UniqueViolation
        }
        rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => DbErrorKind::ForeignKeyViolation,
        rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL => DbErrorKind::NotNullViolation,
        rusqlite::ffi::SQLITE_CONSTRAINT_CHECK => DbErrorKind::CheckViolation,
        // Extended result codes carry the primary code in the low byte.
        c if c & 0xff == 5 || c & 0xff == 6 => DbErrorKind::Deadlock,
        _ => DbErrorKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraint_codes_classify_as_integrity_violations() {
        assert_eq!(kind_of(2067), DbErrorKind::UniqueViolation);
        assert_eq!(kind_of(1555), DbErrorKind::UniqueViolation);
        assert_eq!(kind_of(787), DbErrorKind::ForeignKeyViolation);
        assert_eq!(kind_of(1299), DbErrorKind::NotNullViolation);
        assert_eq!(kind_of(275), DbErrorKind::CheckViolation);
        assert_eq!(kind_of(5), DbErrorKind::Deadlock);
        assert_eq!(kind_of(262), DbErrorKind::Deadlock);
        assert_eq!(kind_of(19), DbErrorKind::Other);
    }
}
