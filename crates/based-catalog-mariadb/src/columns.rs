use based_catalog::{CatalogDiagnostic, Column, TableId, ValueGeneration};
use sqlx::{
    mysql::{MySqlConnection, MySqlRow},
    Row,
};

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
    diagnostics: &mut Vec<CatalogDiagnostic>,
) -> Result<Vec<Column>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/columns.sql")))
        .bind(&id.namespace)
        .bind(&id.name)
        .fetch_all(conn)
        .await?;
    for row in &rows {
        super::column_limits::report(row, id, diagnostics)?;
    }
    rows.iter().map(column).collect()
}

fn column(row: &MySqlRow) -> Result<Column, sqlx::Error> {
    Ok(Column {
        name: row.try_get("COLUMN_NAME")?,
        position: row.try_get("ORDINAL_POSITION")?,
        native_type: super::types::read(row)?,
        nullable: row.try_get::<String, _>("IS_NULLABLE")? == "YES",
        default: row.try_get("COLUMN_DEFAULT")?,
        collation: row.try_get("COLLATION_NAME")?,
        generation: generation(row)?,
    })
}

fn generation(row: &MySqlRow) -> Result<ValueGeneration, sqlx::Error> {
    let extra: String = row.try_get("EXTRA")?;
    if extra.contains("auto_increment") {
        return Ok(ValueGeneration::AutoIncrement);
    }
    if row.try_get::<String, _>("IS_GENERATED")? == "ALWAYS" {
        return Ok(ValueGeneration::Generated {
            expression: row.try_get("GENERATION_EXPRESSION")?,
            stored: extra.contains("STORED") || extra.contains("PERSISTENT"),
        });
    }
    Ok(ValueGeneration::None)
}
