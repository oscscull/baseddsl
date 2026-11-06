use sqlx::error::ErrorKind;

use crate::run::DbErrorKind;

/// The integrity-violation kind of a sqlx error, if it is one. Each sqlx driver already
/// classifies its own server codes (MariaDB 1062, Postgres 23505, SQLite 2067/1555, …).
pub(crate) fn integrity_kind(e: &sqlx::Error) -> Option<DbErrorKind> {
    match e.as_database_error()?.kind() {
        ErrorKind::UniqueViolation => Some(DbErrorKind::UniqueViolation),
        ErrorKind::ForeignKeyViolation => Some(DbErrorKind::ForeignKeyViolation),
        ErrorKind::NotNullViolation => Some(DbErrorKind::NotNullViolation),
        ErrorKind::CheckViolation => Some(DbErrorKind::CheckViolation),
        _ => None,
    }
}
