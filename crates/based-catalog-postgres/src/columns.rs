use based_catalog::{CatalogDiagnostic, Column, TableId, ValueGeneration};
use sqlx::{postgres::PgConnection, Row};

pub(crate) async fn read(
    conn: &mut PgConnection,
    oid: i64,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Vec<Column>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/columns.sql")))
        .bind(oid)
        .fetch_all(&mut *conn)
        .await?;
    let mut columns = Vec::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let expression: Option<String> = row.try_get("expression")?;
        let generation = super::generation::read(&row, expression.clone())?;
        super::generation_limits::report(&generation, id, &name, findings);
        let native_type = super::types::read(conn, &row).await?;
        super::column_limits::report(&row, id, &name, findings)?;
        let position = u32::try_from(row.try_get::<i32, _>("position")?)
            .map_err(|_| sqlx::Error::ColumnNotFound("column ordinal".into()))?;
        let default =
            expression.filter(|_| !matches!(generation, ValueGeneration::Generated { .. }));
        columns.push(Column {
            name,
            position,
            native_type,
            nullable: !row.try_get::<bool, _>("attnotnull")?,
            default,
            collation: row.try_get("collation")?,
            generation,
        });
    }
    Ok(columns)
}
