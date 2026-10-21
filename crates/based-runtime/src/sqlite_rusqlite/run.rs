use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::{Batch, Connection};

use super::bind::bind_from;
use super::decode::row_to_json;
use super::error::map_err;
use crate::run::{DbError, Row};
use crate::value::SqlValue;

/// Run every statement in `sql`, spreading `params` across them, and hand each result row
/// to `on_row` (which returns `false` to stop early). Returns the summed per-statement
/// change counts, as the sqlx driver reports them.
pub(super) fn run(
    conn: &Connection,
    sql: &str,
    params: &[SqlValue],
    mut on_row: impl FnMut(Row) -> bool,
) -> Result<u64, DbError> {
    let mut batch = Batch::new(conn, sql);
    let mut used = 0;
    let mut changes = 0;
    while let Some(mut stmt) = batch.next().map_err(map_err)? {
        used += bind_from(&mut stmt, params, used)?;
        let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
        let mut rows = stmt.raw_query();
        while let Some(row) = rows.next().map_err(map_err)? {
            if !on_row(row_to_json(row, &names)?) {
                return Ok(changes);
            }
        }
        changes += conn.changes();
    }
    Ok(changes)
}
