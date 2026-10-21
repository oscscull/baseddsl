use super::Logged;
use crate::run::{Backend, Db, DbError};
use async_trait::async_trait;

/// A backend whose every checked-out connection logs the statements it runs.
pub struct LoggedBackend<B>(B);

impl<B: Backend> LoggedBackend<B> {
    pub fn new(backend: B) -> Self {
        Self(backend)
    }
}

#[async_trait]
impl<B: Backend> Backend for LoggedBackend<B> {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(Logged(self.0.checkout(shard_key).await?)))
    }

    async fn ping(&self) -> Result<(), DbError> {
        self.0.ping().await
    }
}
