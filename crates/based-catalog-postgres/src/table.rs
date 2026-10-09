use based_catalog::{CatalogDiagnostic, CatalogReadError, Table, TableId, TableKind};
use sqlx::{postgres::PgConnection, Row};

pub(crate) async fn read(
    conn: &mut PgConnection,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Option<Table>, CatalogReadError> {
    load(conn, id, findings)
        .await
        .map_err(|_| CatalogReadError::Metadata)
}

async fn load(
    conn: &mut PgConnection,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Option<Table>, sqlx::Error> {
    let row = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/table.sql")))
        .bind(&id.namespace)
        .bind(&id.name)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !row.try_get::<bool, _>("visible")? {
        return Err(sqlx::Error::ColumnNotFound(
            "schema metadata visibility".into(),
        ));
    }
    let kind = match row.try_get::<String, _>("kind")?.as_str() {
        "r" | "p" => TableKind::Table,
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::VirtualTable,
        _ => return Err(sqlx::Error::ColumnNotFound("object kind".into())),
    };
    let oid: i64 = row.try_get("oid")?;
    super::table_limits::report(&row, id, findings)?;
    super::constraint_limits::report(conn, oid, id, findings).await?;
    let columns = super::columns::read(conn, oid, id, findings).await?;
    let (primary_key, unique_keys) = super::keys::read(conn, oid).await?;
    let indexes = super::indexes::read(conn, oid, id, findings).await?;
    let foreign_keys = super::foreign_keys::read(conn, oid).await?;
    let checks = super::checks::read(conn, oid).await?;
    if usize::try_from(row.try_get::<i16, _>("relchecks")?)
        .map_err(|_| sqlx::Error::ColumnNotFound("CHECK count".into()))?
        != checks.len()
    {
        return Err(sqlx::Error::ColumnNotFound(
            "incomplete CHECK metadata".into(),
        ));
    }
    let native_definition = match kind {
        TableKind::View | TableKind::MaterializedView => Some(
            sqlx::query_scalar(sqlx::AssertSqlSafe(
                "SELECT pg_catalog.pg_get_viewdef($1::bigint::oid,false)",
            ))
            .bind(oid)
            .fetch_one(conn)
            .await?,
        ),
        _ => None,
    };
    Ok(Some(Table {
        id: id.clone(),
        kind,
        columns,
        primary_key,
        unique_keys,
        indexes,
        foreign_keys,
        checks,
        native_definition,
        engine: None,
        without_rowid: false,
        strict: false,
    }))
}
