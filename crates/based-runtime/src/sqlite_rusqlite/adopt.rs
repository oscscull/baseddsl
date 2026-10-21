use async_trait::async_trait;
use rusqlite::Connection;

use super::run::run;
use crate::run::{DbError, DbRead, Row, RowStream};
use crate::value::SqlValue;

/// A borrowed adapter over a **caller-owned** rusqlite connection with an open
/// transaction: the bring-your-own (`adopt`) rung. Work runs inside the caller's
/// transaction and commits atomically with the caller's own writes. It never begins,
/// commits, or rolls back; the caller does (e.g. `execute_batch("BEGIN")` … `"COMMIT"`).
///
/// Statements run on the calling task, blocking it for their duration: the connection
/// stays with its owner. A call on a connection with no open transaction fails rather
/// than silently autocommitting.
pub struct AdoptedRusqlite<'a> {
    conn: &'a mut Connection,
}

impl<'a> AdoptedRusqlite<'a> {
    /// Adopt a caller-owned connection that has a transaction open.
    pub fn new(conn: &'a mut Connection) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl DbRead for AdoptedRusqlite<'_> {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        // Held as `&mut` (`Send`); the shared borrow exists only inside the sync call.
        let conn = &mut *self.conn;
        let params = params.to_vec();
        Box::pin(async_stream::stream! {
            for item in collect(conn, sql, &params) {
                yield item;
            }
        })
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        in_transaction(self.conn)?;
        run(self.conn, sql, params, |_| true)
    }
}

/// Every row of a query, then its error if it failed part-way.
fn collect(conn: &Connection, sql: &str, params: &[SqlValue]) -> Vec<Result<Row, DbError>> {
    let mut rows = Vec::new();
    let outcome = in_transaction(conn).and_then(|()| {
        run(conn, sql, params, |row| {
            rows.push(Ok(row));
            true
        })
    });
    if let Err(e) = outcome {
        rows.push(Err(e));
    }
    rows
}

fn in_transaction(conn: &Connection) -> Result<(), DbError> {
    if conn.is_autocommit() {
        return Err(DbError::new(
            "the adopted sqlite connection has no open transaction",
        ));
    }
    Ok(())
}
