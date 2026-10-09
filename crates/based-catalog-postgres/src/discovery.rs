//! Own one repeatable-read, read-only catalog snapshot; cancellation queues rollback.
use super::PostgresCatalogReader;
use based_catalog::{Catalog, CatalogReadError, CatalogReader, Discovery, Selection};
use sqlx::Connection;

impl CatalogReader for PostgresCatalogReader {
    async fn discover(&mut self, selection: &Selection) -> Result<Discovery, CatalogReadError> {
        let mut transaction = self
            .connection
            .begin_with(sqlx::AssertSqlSafe(
                "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
            ))
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        let mut tables = Vec::new();
        let mut findings = Vec::new();
        for id in selection.tables() {
            if let Some(table) = super::table::read(&mut transaction, id, &mut findings).await? {
                tables.push(table);
            }
        }
        transaction
            .rollback()
            .await
            .map_err(|_| CatalogReadError::Metadata)?;
        Ok(Discovery::checked(
            Catalog {
                source: self.source.clone(),
                tables,
            },
            selection,
            findings,
        ))
    }
}
