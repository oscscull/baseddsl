use based_catalog::Key;
use sqlx::{postgres::PgConnection, Row};
use std::collections::BTreeMap;

pub(crate) async fn read(
    conn: &mut PgConnection,
    oid: i64,
) -> Result<(Option<Key>, Vec<Key>), sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/keys.sql")))
        .bind(oid)
        .fetch_all(conn)
        .await?;
    let mut keys = BTreeMap::<String, (bool, Key)>::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let primary = row.try_get::<String, _>("kind")? == "p";
        let deferral = super::constraints::deferral(&row)?;
        keys.entry(name.clone())
            .or_insert_with(|| {
                (
                    primary,
                    Key {
                        name: Some(name),
                        columns: Vec::new(),
                        deferral,
                    },
                )
            })
            .1
            .columns
            .push(row.try_get("column_name")?);
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
