//! Per-dialect numeric casts that preserve aggregate wire types.

use super::Dialect;

/// The `CAST(… AS <int>)` target that coerces a widened `SUM(int)` back to an integer, or
/// `None` where the dialect keeps it integral (SQLite). MariaDB/Postgres widen `SUM` of a
/// `BIGINT` to decimal/numeric, which would decode as a string; the cast keeps it a number.
pub(crate) fn int_cast_type(dialect: Dialect) -> Option<&'static str> {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => Some("SIGNED"),
        Dialect::Postgres => Some("BIGINT"),
        Dialect::Sqlite => None,
    }
}

/// The dialect's double type, the `CAST` target that makes `AVG` decode as a float number
/// on every dialect (Postgres `AVG` of an int/numeric is otherwise a numeric string).
pub(crate) fn double_cast_type(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::MariaDb | Dialect::MySql => "DOUBLE",
        Dialect::Postgres => "DOUBLE PRECISION",
        Dialect::Sqlite => "REAL",
    }
}
