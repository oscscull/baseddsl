use async_trait::async_trait;

use super::execute::execute;
use super::fetch::fetch;
use super::pool::Pooled;
use crate::run::{DbError, DbRead, RowStream, Tx};
use crate::value::SqlValue;

/// An open transaction on one pooled connection. Dropped without commit (or mid-begin),
/// it rolls back before the connection can serve anyone else.
pub(super) struct RusqliteTx {
    pub(super) conn: Pooled,
}

impl Drop for RusqliteTx {
    fn drop(&mut self) {
        // Queued ahead of the pool return, so the next checkout finds no open transaction.
        let _ = self.conn.submit(Box::new(|conn| {
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
        }));
    }
}

#[async_trait]
impl DbRead for RusqliteTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        fetch(&self.conn, sql, params)
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        execute(&self.conn, sql, params).await
    }
}

#[async_trait]
impl Tx for RusqliteTx {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        execute(&self.conn, "COMMIT", &[]).await.map(|_| ())
    }

    async fn rollback(self: Box<Self>) -> Result<(), DbError> {
        execute(&self.conn, "ROLLBACK", &[]).await.map(|_| ())
    }
}
