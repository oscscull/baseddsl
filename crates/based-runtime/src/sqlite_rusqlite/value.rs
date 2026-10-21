use rusqlite::types::Value;

use crate::value::SqlValue;

/// The SQLite value a [`SqlValue`] binds as: a `bool` is integer `0/1`, every text-riding
/// family its wire string, `json` serialized text, and `bytes` the decoded raw bytes (a
/// malformed base64 string binds as empty, as on the sqlx driver).
pub(super) fn to_value(v: &SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Int(i) => Value::Integer(*i),
        SqlValue::Float(f) => Value::Real(*f),
        SqlValue::Bool(b) => Value::Integer(i64::from(*b)),
        SqlValue::Text(s)
        | SqlValue::Uuid(s)
        | SqlValue::Timestamp(s)
        | SqlValue::Date(s)
        | SqlValue::Time(s)
        | SqlValue::Decimal(s) => Value::Text(s.clone()),
        SqlValue::Bytes(s) => Value::Blob(crate::value::b64_decode(s).unwrap_or_default()),
        SqlValue::Json(j) => Value::Text(j.to_string()),
        // A list is expanded to per-element binds by `to_positional` before this point.
        SqlValue::List(_) => unreachable!("list values are expanded before binding"),
    }
}
