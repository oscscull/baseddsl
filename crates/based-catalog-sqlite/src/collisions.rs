//! Real tables must never shadow the metadata virtual-table authorizer allowlist.
use sqlx::SqliteConnection;

pub(crate) async fn require(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let exists: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE lower(name) IN ('pragma_table_list','pragma_table_xinfo','pragma_index_list','pragma_index_xinfo','pragma_foreign_key_list'))"))
        .fetch_one(conn).await?;
    if exists != 0 {
        return Err(sqlx::Error::Protocol(
            "Metadata virtual-table name is shadowed".into(),
        ));
    }
    Ok(())
}
