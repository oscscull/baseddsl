//! Admin-only before/after evidence, separate from the metadata reader.
use sqlx::{mysql::MySqlConnection, Row};

pub async fn read(conn: &mut MySqlConnection) -> Vec<String> {
    let rows = sqlx::query(sqlx::AssertSqlSafe("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA='based_catalog_fixture' ORDER BY TABLE_NAME"))
        .fetch_all(&mut *conn).await.unwrap();
    let mut facts = Vec::new();
    for row in rows {
        let name: String = row.get("TABLE_NAME");
        let sql = format!(
            "SHOW CREATE TABLE based_catalog_fixture.`{}`",
            name.replace('`', "``")
        );
        facts.push(
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .fetch_one(&mut *conn)
                .await
                .unwrap()
                .get::<String, _>(1),
        );
    }
    facts.push(
        sqlx::query_scalar(sqlx::AssertSqlSafe(
            "SELECT CONCAT(label, ':', guessed_id) FROM based_catalog_fixture.keyless",
        ))
        .fetch_one(&mut *conn)
        .await
        .unwrap(),
    );
    let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT COUNT(*) FROM based_catalog_fixture.`odd`` table` WHERE `part z` = 7 AND `part a` = 9"))
        .fetch_one(conn).await.unwrap();
    facts.push(count.to_string());
    facts
}
