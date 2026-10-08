//! Require a whole-table grant rather than treating hidden columns as a complete catalog.
use based_catalog::TableId;
use sqlx::{mysql::MySqlConnection, Row};

pub(crate) async fn require(conn: &mut MySqlConnection, id: &TableId) -> Result<(), sqlx::Error> {
    let account: String = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT CURRENT_USER()"))
        .fetch_one(&mut *conn)
        .await?;
    let (user, host) = account
        .rsplit_once('@')
        .ok_or_else(|| sqlx::Error::ColumnNotFound("account identity".into()))?;
    let grantee = format!(
        "'{}'@'{}'",
        user.replace('\'', "''"),
        host.replace('\'', "''")
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/visibility.sql")))
        .bind(&grantee)
        .bind(&grantee)
        .bind(&id.namespace)
        .bind(&grantee)
        .bind(&id.namespace)
        .bind(&id.name)
        .fetch_one(conn)
        .await?;
    if row.try_get::<i64, _>("visible")? == 0 {
        return Err(sqlx::Error::ColumnNotFound(
            "whole-table metadata grant required".into(),
        ));
    }
    Ok(())
}
