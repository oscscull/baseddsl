//! Untimed capture of the actual executed statements. Timed engines have no wrapper.
use async_trait::async_trait;
use based_runtime::{
    run::{Backend, Db, DbError, DbRead, RowStream, Tx},
    value::SqlValue,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<SqlValue>,
}
pub type Log = Arc<Mutex<Vec<Statement>>>;
pub struct Capture<B> {
    pub inner: B,
    pub log: Log,
}
struct Connection<T: ?Sized> {
    inner: Box<T>,
    log: Log,
}

impl<T: ?Sized> Connection<T> {
    fn record(&self, sql: &str, params: &[SqlValue]) {
        self.log.lock().unwrap().push(Statement {
            sql: sql.into(),
            params: params.to_vec(),
        });
    }
}

#[async_trait]
impl<T: DbRead + ?Sized> DbRead for Connection<T> {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        self.record(sql, params);
        self.inner.fetch(sql, params)
    }
    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.record(sql, params);
        self.inner.execute(sql, params).await
    }
}

#[async_trait]
impl Db for Connection<dyn Db> {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        Ok(Box::new(Connection {
            inner: self.inner.begin().await?,
            log: self.log,
        }))
    }
}

#[async_trait]
impl Tx for Connection<dyn Tx> {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        self.inner.commit().await
    }
    async fn rollback(self: Box<Self>) -> Result<(), DbError> {
        self.inner.rollback().await
    }
}

#[async_trait]
impl<B: Backend> Backend for Capture<B> {
    async fn checkout(&self, key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(Connection {
            inner: self.inner.checkout(key).await?,
            log: self.log.clone(),
        }))
    }
}
