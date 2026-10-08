use based_catalog::{Column, ValueGeneration};
use sqlx::{sqlite::SqliteRow, Row, SqliteConnection};

pub(crate) struct Columns {
    pub columns: Vec<Column>,
    pub primary_parts: Vec<(i64, String)>,
}

pub(crate) async fn read(
    conn: &mut SqliteConnection,
    name: &str,
    without_rowid: bool,
    strict: bool,
    has_collation: bool,
) -> Result<Columns, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/columns.sql")))
        .bind(name)
        .fetch_all(conn)
        .await?;
    let mut primary_parts = Vec::new();
    let mut columns = Vec::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let primary: i64 = row.try_get("pk")?;
        if primary > 0 {
            primary_parts.push((primary, name.clone()));
        }
        let position = u32::try_from(row.try_get::<i64, _>("cid")?)
            .map_err(|_| sqlx::Error::ColumnNotFound("column ordinal".into()))?;
        let nullable =
            row.try_get::<i64, _>("required")? == 0 && !(primary > 0 && (without_rowid || strict));
        columns.push(Column {
            name,
            position,
            native_type: super::types::read(row.try_get("type")?, strict),
            nullable,
            default: row.try_get("dflt_value")?,
            collation: (!has_collation).then(|| "BINARY".into()),
            generation: generation(&row)?,
        });
    }
    primary_parts.sort_by_key(|(position, _)| *position);
    Ok(Columns {
        columns,
        primary_parts,
    })
}

fn generation(row: &SqliteRow) -> Result<ValueGeneration, sqlx::Error> {
    match row.try_get::<i64, _>("hidden")? {
        2 => Ok(ValueGeneration::Generated {
            expression: None,
            stored: false,
        }),
        3 => Ok(ValueGeneration::Generated {
            expression: None,
            stored: true,
        }),
        _ => Ok(ValueGeneration::None),
    }
}
