use super::line::log_statement;
use crate::run::{Db, DbError, DbRead, RowStream, Tx};
use crate::tx::TxOptions;
use crate::value::SqlValue;
use async_trait::async_trait;
use std::ops::DerefMut;

/// A connection, transaction, or borrowed handle that logs each statement it runs.
pub struct Logged<T>(pub T);

#[async_trait]
impl<T> DbRead for Logged<T>
where
    T: DerefMut + Send,
    T::Target: DbRead,
{
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        log_statement(sql, params.len());
        self.0.fetch(sql, params)
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        log_statement(sql, params.len());
        self.0.execute(sql, params).await
    }
}

#[async_trait]
impl Db for Logged<Box<dyn Db>> {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        Ok(Box::new(Logged(self.0.begin().await?)))
    }

    async fn begin_tx(self: Box<Self>, opts: TxOptions) -> Result<Box<dyn Tx>, DbError> {
        Ok(Box::new(Logged(self.0.begin_tx(opts).await?)))
    }
}

#[async_trait]
impl Tx for Logged<Box<dyn Tx>> {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        self.0.commit().await
    }

    async fn rollback(self: Box<Self>) -> Result<(), DbError> {
        self.0.rollback().await
    }
}
