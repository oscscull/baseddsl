use async_trait::async_trait;

use super::begin::begin;
use super::execute::execute;
use super::fetch::fetch;
use super::pool::Pooled;
use super::tx::RusqliteTx;
use crate::run::{Db, DbError, DbRead, RowStream, Tx};
use crate::value::SqlValue;

/// One pooled rusqlite connection, running one request.
pub struct RusqliteDb {
    pub(super) conn: Pooled,
}

#[async_trait]
impl DbRead for RusqliteDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        fetch(&self.conn, sql, params)
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        execute(&self.conn, sql, params).await
    }
}

#[async_trait]
impl Db for RusqliteDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        // The guard exists before `BEGIN` runs, so a cancelled begin still rolls back.
        let tx = RusqliteTx { conn: self.conn };
        begin(&tx.conn, None).await?;
        Ok(Box::new(tx))
    }

    async fn begin_tx(self: Box<Self>, opts: crate::tx::TxOptions) -> Result<Box<dyn Tx>, DbError> {
        // SQLite has no SQL-standard isolation; the intent maps to the `BEGIN` locking mode.
        let plan =
            based_codegen::Dialect::Sqlite.begin_transaction_sql(opts.isolation, opts.access);
        if let Some(pre) = plan.pre {
            execute(&self.conn, &pre, &[]).await?;
        }
        let tx = RusqliteTx { conn: self.conn };
        begin(&tx.conn, plan.begin.as_deref()).await?;
        Ok(Box::new(tx))
    }
}
