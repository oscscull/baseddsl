use rusqlite::types::ValueRef;
use serde_json::Value as J;

use super::error::map_err;
use crate::run::{DbError, Row};

/// One result row → JSON, each value read by its own storage class (SQLite types per
/// value, not per column): `TEXT` verbatim, `BLOB` as base64, numbers as numbers.
pub(super) fn row_to_json(row: &rusqlite::Row<'_>, names: &[String]) -> Result<Row, DbError> {
    let mut obj = serde_json::Map::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        let value = decode(row.get_ref(i).map_err(map_err)?)
            .map_err(|e| DbError::new(format!("decoding column `{name}`: {e}")))?;
        obj.insert(name.clone(), value);
    }
    Ok(obj)
}

fn decode(v: ValueRef<'_>) -> Result<J, std::str::Utf8Error> {
    Ok(match v {
        ValueRef::Null => J::Null,
        ValueRef::Integer(i) => J::Number(i.into()),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(J::Null, J::Number),
        ValueRef::Text(t) => J::String(std::str::from_utf8(t)?.to_owned()),
        ValueRef::Blob(b) => J::String(crate::value::b64_encode(b)),
    })
}
