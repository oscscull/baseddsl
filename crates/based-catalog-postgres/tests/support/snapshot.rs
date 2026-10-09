//! Administrator-only schema/data/sequence evidence; never used by discovery.
use sqlx::{postgres::PgConnection, Row};

pub async fn read(conn: &mut PgConnection) -> Vec<String> {
    let rows = sqlx::query(sqlx::AssertSqlSafe("SELECT n.nspname::text AS schema_name,c.relname::text AS table_name,a.attname::text AS column_name,pg_catalog.format_type(a.atttypid,a.atttypmod) AS declaration,a.attnotnull,pg_catalog.pg_get_expr(d.adbin,d.adrelid,false) AS expression FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE n.nspname IN ('catalog fixture','catalog_other') AND a.attnum > 0 AND NOT a.attisdropped ORDER BY n.nspname,c.relname,a.attnum"))
        .fetch_all(&mut *conn).await.unwrap();
    let mut facts: Vec<String> = rows
        .iter()
        .map(|row| {
            format!(
                "{}:{}:{}:{}:{}:{:?}",
                row.get::<String, _>("schema_name"),
                row.get::<String, _>("table_name"),
                row.get::<String, _>("column_name"),
                row.get::<String, _>("declaration"),
                row.get::<bool, _>("attnotnull"),
                row.get::<Option<String>, _>("expression")
            )
        })
        .collect();
    for sql in [
        "SELECT pg_catalog.pg_get_constraintdef(c.oid,false) FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_namespace n ON n.oid=c.connamespace WHERE n.nspname IN ('catalog fixture','catalog_other') ORDER BY n.nspname,c.conname",
        "SELECT pg_catalog.pg_get_indexdef(c.oid,0,false) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN ('catalog fixture','catalog_other') AND c.relkind='i' ORDER BY n.nspname,c.relname",
        "SELECT enumlabel::text FROM pg_catalog.pg_enum e JOIN pg_catalog.pg_type t ON t.oid=e.enumtypid JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='catalog fixture' ORDER BY e.enumsortorder",
        "SELECT label || ':' || guessed_id::text FROM \"catalog fixture\".keyless",
        "SELECT \"part z\"::text || ':' || \"part a\"::text FROM \"catalog fixture\".\"odd\"\" table\"",
        "SELECT sequencename::text || ':' || COALESCE(last_value::text,'unused') FROM pg_catalog.pg_sequences WHERE schemaname='catalog fixture' ORDER BY sequencename",
    ] {
        let values: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql)).fetch_all(&mut *conn).await.unwrap();
        facts.extend(values);
    }
    facts
}
