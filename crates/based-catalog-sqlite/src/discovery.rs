//! Own one read-only file snapshot and its rollback lifetime.
use super::SqliteCatalogReader;
use based_catalog::{Catalog, CatalogReadError, CatalogReader, Discovery, Selection};
use sqlx::Connection;

impl CatalogReader for SqliteCatalogReader {
    async fn discover(&mut self, selection: &Selection) -> Result<Discovery, CatalogReadError> {
        let mut transaction = self
            .connection
            .begin()
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
