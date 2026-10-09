use based_catalog::{CheckConstraint, TableId};
use sqlx::{mysql::MySqlConnection, Row};

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
) -> Result<Vec<CheckConstraint>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe("SELECT CONSTRAINT_NAME, CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = ? AND TABLE_NAME = ? ORDER BY CONSTRAINT_NAME"))
        .bind(&id.namespace).bind(&id.name).fetch_all(conn).await?;
    rows.iter()
        .map(|row| {
            Ok(CheckConstraint {
                name: Some(row.try_get("CONSTRAINT_NAME")?),
                expression: row.try_get("CHECK_CLAUSE")?,
            })
        })
        .collect()
}
