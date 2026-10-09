//! Own the read-only transaction lifetime, including cancellation rollback.
use super::MariaDbCatalogReader;
use based_catalog::{CatalogReadError, CatalogReader, Discovery, Selection};
use sqlx::Connection;

impl CatalogReader for MariaDbCatalogReader {
    async fn discover(&mut self, selection: &Selection) -> Result<Discovery, CatalogReadError> {
        let mut transaction = self
            .connection
            .begin_with(sqlx::AssertSqlSafe("START TRANSACTION READ ONLY"))
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        let result = super::snapshot::stable(&mut transaction, &self.source, selection).await;
        transaction
            .rollback()
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        result
    }
}
