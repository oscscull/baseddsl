use based_catalog::{Deferral, Key, TableId};
use sqlx::{mysql::MySqlConnection, Row};
use std::collections::BTreeMap;

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
) -> Result<(Option<Key>, Vec<Key>), sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/keys.sql")))
        .bind(&id.namespace)
        .bind(&id.name)
        .fetch_all(conn)
        .await?;
    let mut keys = BTreeMap::<String, (bool, Key)>::new();
    for row in rows {
        let name: String = row.try_get("CONSTRAINT_NAME")?;
        let primary = row.try_get::<String, _>("CONSTRAINT_TYPE")? == "PRIMARY KEY";
        keys.entry(name.clone())
            .or_insert_with(|| {
                (
                    primary,
                    Key {
                        name: Some(name),
                        columns: Vec::new(),
                        deferral: Deferral::NotDeferrable,
                    },
                )
            })
            .1
            .columns
            .push(row.try_get("COLUMN_NAME")?);
    }
    let mut primary = None;
    let mut unique = Vec::new();
    for (is_primary, key) in keys.into_values() {
        if is_primary {
            primary = Some(key);
            continue;
        }
        unique.push(key);
    }
    Ok((primary, unique))
}
