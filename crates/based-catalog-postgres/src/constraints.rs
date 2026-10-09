//! Normalize PostgreSQL constraint timing, shared by declared keys and foreign keys.
use based_catalog::Deferral;
use sqlx::{postgres::PgRow, Row};

pub(crate) fn deferral(row: &PgRow) -> Result<Deferral, sqlx::Error> {
    if !row.try_get::<bool, _>("condeferrable")? {
        return Ok(Deferral::NotDeferrable);
    }
    if row.try_get("condeferred")? {
        return Ok(Deferral::InitiallyDeferred);
    }
    Ok(Deferral::InitiallyImmediate)
}
