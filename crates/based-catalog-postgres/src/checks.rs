use based_catalog::CheckConstraint;
use sqlx::{postgres::PgConnection, Row};

pub(crate) async fn read(
    conn: &mut PgConnection,
    oid: i64,
) -> Result<Vec<CheckConstraint>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe("SELECT conname::text AS name, pg_catalog.pg_get_expr(conbin,conrelid,false) AS expression FROM pg_catalog.pg_constraint WHERE conrelid = $1::bigint::oid AND contype = 'c' ORDER BY conname"))
        .bind(oid).fetch_all(conn).await?;
    rows.iter()
        .map(|row| {
            Ok(CheckConstraint {
                name: Some(row.try_get("name")?),
                expression: row.try_get("expression")?,
            })
        })
        .collect()
}
