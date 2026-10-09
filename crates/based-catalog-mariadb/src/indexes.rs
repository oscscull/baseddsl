use based_catalog::{Index, IndexDirection, IndexOrigin, IndexPart, IndexTarget, Key, TableId};
use sqlx::{mysql::MySqlConnection, Row};
use std::collections::BTreeMap;

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
    keys: &[Key],
) -> Result<Vec<Index>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/indexes.sql")))
        .bind(&id.namespace)
        .bind(&id.name)
        .fetch_all(conn)
        .await?;
    let mut indexes = BTreeMap::<String, Index>::new();
    for row in rows {
        let name: String = row.try_get("INDEX_NAME")?;
        let unique = row.try_get::<i64, _>("NON_UNIQUE")? == 0;
        let method: String = row.try_get("INDEX_TYPE")?;
        let valid = row.try_get::<String, _>("IGNORED")? == "NO";
        let origin = origin(&name, keys);
        let index = indexes.entry(name.clone()).or_insert_with(|| Index {
            name,
            unique,
            origin,
            method: Some(method),
            valid,
            parts: Vec::new(),
            included_columns: Vec::new(),
            predicate: None,
            native_definition: None,
        });
        index.parts.push(IndexPart {
            target: row
                .try_get::<Option<String>, _>("COLUMN_NAME")?
                .map_or(IndexTarget::Expression(None), IndexTarget::Column),
            direction: match row.try_get::<Option<String>, _>("COLLATION")?.as_deref() {
                Some("D") => IndexDirection::Descending,
                _ => IndexDirection::Ascending,
            },
            prefix_length: row
                .try_get::<Option<i64>, _>("SUB_PART")?
                .map(u32::try_from)
                .transpose()
                .map_err(|_| sqlx::Error::ColumnNotFound("index prefix overflow".into()))?,
            collation: None,
        });
    }
    Ok(indexes.into_values().collect())
}

fn origin(name: &str, keys: &[Key]) -> IndexOrigin {
    if name == "PRIMARY" {
        return IndexOrigin::PrimaryKey;
    }
    if keys.iter().any(|key| key.name.as_deref() == Some(name)) {
        return IndexOrigin::UniqueConstraint;
    }
    IndexOrigin::Explicit
}
