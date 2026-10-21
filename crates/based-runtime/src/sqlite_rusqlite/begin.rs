use rusqlite::Connection;

use super::run::run;
use super::worker::Worker;
use crate::run::DbError;

/// Open a transaction with `statement` (plain `BEGIN` when `None`). A custom statement
/// must leave a transaction open, as sqlx requires.
pub(super) async fn begin(worker: &Worker, statement: Option<&str>) -> Result<(), DbError> {
    let statement = statement.map(str::to_owned);
    worker
        .call(move |conn| open(conn, statement.as_deref()))
        .await?
}

fn open(conn: &Connection, statement: Option<&str>) -> Result<(), DbError> {
    run(conn, statement.unwrap_or("BEGIN"), &[], |_| true)?;
    if conn.is_autocommit() {
        return Err(DbError::new("BEGIN statement did not start a transaction"));
    }
    Ok(())
}
