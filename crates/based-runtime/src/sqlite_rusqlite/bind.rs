use rusqlite::Statement;

use super::error::map_err;
use super::value::to_value;
use crate::run::DbError;
use crate::value::SqlValue;

/// Bind `params` onto one statement of a batch, anonymous `?`s continuing from `offset`
/// (a batch spreads one parameter list across its statements). `?NNN`/`$NNN` index the
/// list directly; a parameter past its end stays NULL, as SQLite and the sqlx driver
/// leave it. Returns how many anonymous parameters this statement consumed.
pub(super) fn bind_from(
    stmt: &mut Statement<'_>,
    params: &[SqlValue],
    offset: usize,
) -> Result<usize, DbError> {
    let mut next = offset;
    for slot in 1..=stmt.parameter_count() {
        let n = if let Some(name) = stmt.parameter_name(slot) {
            numbered(name)?
        } else {
            next += 1;
            next
        };
        let Some(value) = n.checked_sub(1).and_then(|i| params.get(i)) else {
            break;
        };
        stmt.raw_bind_parameter(slot, to_value(value))
            .map_err(map_err)?;
    }
    Ok(next - offset)
}

/// The 1-based index a `?NNN` / `$NNN` parameter names.
fn numbered(name: &str) -> Result<usize, DbError> {
    name.strip_prefix('?')
        .or_else(|| name.strip_prefix('$'))
        .and_then(|digits| digits.parse().ok())
        .ok_or_else(|| DbError::new(format!("unsupported SQL parameter format: {name}")))
}
