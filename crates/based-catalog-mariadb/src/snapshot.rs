//! Collect a closed selection twice to detect catalog changes; no row sampling.
use based_catalog::{Catalog, CatalogReadError, CatalogSource, Discovery, Selection};
use sqlx::mysql::MySqlConnection;

pub(crate) async fn stable(
    conn: &mut MySqlConnection,
    source: &CatalogSource,
    selection: &Selection,
) -> Result<Discovery, CatalogReadError> {
    let first = read(conn, source, selection).await?;
    let second = read(conn, source, selection).await?;
    if first != second {
        return Err(CatalogReadError::InconsistentSnapshot);
    }
    Ok(first)
}

async fn read(
    conn: &mut MySqlConnection,
    source: &CatalogSource,
    selection: &Selection,
) -> Result<Discovery, CatalogReadError> {
    let mut tables = Vec::new();
    let mut diagnostics = Vec::new();
    for id in selection.tables() {
        if let Some(table) = super::table::read(conn, id, &mut diagnostics).await? {
            tables.push(table);
        }
    }
    Ok(Discovery::checked(
        Catalog {
            source: source.clone(),
            tables,
        },
        selection,
        diagnostics,
    ))
}
