use super::keywords::Keywords;
use based_catalog::{CatalogDiagnostic, CatalogReadError, Table, TableId, TableKind};
use sqlx::{Row, SqliteConnection};

pub(crate) async fn read(
    conn: &mut SqliteConnection,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Option<Table>, CatalogReadError> {
    load(conn, id, findings)
        .await
        .map_err(|_| CatalogReadError::Metadata)
}

async fn load(
    conn: &mut SqliteConnection,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Option<Table>, sqlx::Error> {
    if id.namespace != "main" {
        return Err(sqlx::Error::ColumnNotFound(
            "only main file catalog is supported".into(),
        ));
    }
    let row = sqlx::query(sqlx::AssertSqlSafe(
        "SELECT type, wr, strict FROM pragma_table_list WHERE schema = 'main' AND name = ?",
    ))
    .bind(&id.name)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let kind = match row.try_get::<String, _>("type")?.as_str() {
        "table" => TableKind::Table,
        "view" => TableKind::View,
        "virtual" | "shadow" => TableKind::VirtualTable,
        _ => return Err(sqlx::Error::ColumnNotFound("object kind".into())),
    };
    let native_definition: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(
        "SELECT sql FROM main.sqlite_schema WHERE name = ? AND type IN ('table','view')",
    ))
    .bind(&id.name)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let keywords = Keywords::read(native_definition.as_deref().unwrap_or_default());
    let without_rowid = row.try_get::<i64, _>("wr")? != 0;
    let strict = row.try_get::<i64, _>("strict")? != 0;
    if kind != TableKind::Table {
        return Ok(Some(super::opaque::object(
            id,
            kind,
            native_definition,
            findings,
        )));
    }
    let mut columns = super::columns::read(
        conn,
        &id.name,
        without_rowid,
        strict,
        keywords.has("COLLATE"),
    )
    .await?;
    let indexes = super::indexes::read(conn, &id.name, id, findings).await?;
    let primary_key = super::keys::primary(columns.primary_parts);
    let unique_keys = super::keys::unique(&indexes)?;
    super::rowid::apply(
        &mut columns.columns,
        &primary_key,
        &indexes,
        without_rowid,
        keywords.has("AUTOINCREMENT"),
    );
    let foreign_keys = super::foreign_keys::read(
        conn,
        &id.name,
        keywords.has("DEFERRABLE") || keywords.has("INITIALLY"),
    )
    .await?;
    let table = Table {
        id: id.clone(),
        kind,
        columns: columns.columns,
        primary_key,
        unique_keys,
        indexes,
        foreign_keys,
        checks: Vec::new(),
        native_definition,
        engine: None,
        without_rowid,
        strict,
    };
    super::limitations::report(&table, &keywords, findings);
    Ok(Some(table))
}
