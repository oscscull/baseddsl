//! Retain native definitions without application-row queries.
use based_catalog::{TableId, TableKind};
use sqlx::{mysql::MySqlConnection, Row};

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
    kind: TableKind,
) -> Result<Option<String>, sqlx::Error> {
    if kind == TableKind::View {
        let definition: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT VIEW_DEFINITION FROM information_schema.VIEWS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?"))
            .bind(&id.namespace).bind(&id.name).fetch_optional(conn).await?;
        return Ok(definition.filter(|definition| !definition.is_empty()));
    }
    // Only identifiers are interpolated; doubling backticks is MariaDB identifier quoting.
    let quoted = |name: &str| format!("`{}`", name.replace('`', "``"));
    let sql = format!(
        "SHOW CREATE TABLE {}.{}",
        quoted(&id.namespace),
        quoted(&id.name)
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_one(conn)
        .await?;
    Ok(Some(row.try_get(1)?))
}
