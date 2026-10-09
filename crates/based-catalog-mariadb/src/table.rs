use based_catalog::{CatalogDiagnostic, CatalogReadError, Table, TableId, TableKind};
use sqlx::{mysql::MySqlConnection, Row};

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) -> Result<Option<Table>, CatalogReadError> {
    load(conn, id, diagnostics)
        .await
        .map_err(|_| CatalogReadError::Metadata)
}

async fn load(
    conn: &mut MySqlConnection,
    id: &TableId,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) -> Result<Option<Table>, sqlx::Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe("SELECT TABLE_TYPE, ENGINE, CREATE_OPTIONS FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?"))
        .bind(&id.namespace).bind(&id.name).fetch_optional(&mut *conn).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    super::visibility::require(conn, id).await?;
    let kind = match row.try_get::<String, _>("TABLE_TYPE")?.as_str() {
        "BASE TABLE" => TableKind::Table,
        "VIEW" => TableKind::View,
        _ => return Err(sqlx::Error::ColumnNotFound("unsupported table kind".into())),
    };
    let engine: Option<String> = row.try_get("ENGINE")?;
    let options = row
        .try_get::<Option<String>, _>("CREATE_OPTIONS")?
        .unwrap_or_default();
    let definition = super::definition::read(conn, id, kind).await?;
    let columns = super::columns::read(conn, id, diagnostics).await?;
    let (primary_key, unique_keys) = super::keys::read(conn, id).await?;
    let indexes = super::indexes::read(conn, id, &unique_keys).await?;
    let foreign_keys = super::foreign_keys::read(conn, id).await?;
    let checks = super::checks::read(conn, id).await?;
    let table = Table {
        id: id.clone(),
        kind,
        columns,
        primary_key,
        unique_keys,
        indexes,
        foreign_keys,
        checks,
        native_definition: definition,
        engine,
        without_rowid: false,
        strict: false,
    };
    super::limitations::report(&table, &options, diagnostics);
    Ok(Some(table))
}
