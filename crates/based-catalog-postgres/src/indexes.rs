use based_catalog::{
    CatalogDiagnostic, Index, IndexDirection, IndexOrigin, IndexPart, IndexTarget, TableId,
};
use sqlx::{
    postgres::{PgConnection, PgRow},
    Row,
};
use std::collections::{btree_map::Entry, BTreeMap};

pub(crate) async fn read(
    conn: &mut PgConnection,
    oid: i64,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<Vec<Index>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/indexes.sql")))
        .bind(oid)
        .fetch_all(conn)
        .await?;
    let mut indexes = BTreeMap::<String, Index>::new();
    for row in rows {
        super::index_limits::report(&row, id, findings)?;
        let name: String = row.try_get("name")?;
        let index = match indexes.entry(name) {
            Entry::Vacant(entry) => entry.insert(index(&row)?),
            Entry::Occupied(entry) => entry.into_mut(),
        };
        let column: Option<String> = row.try_get("column_name")?;
        if row.try_get::<i32, _>("position")? > row.try_get::<i32, _>("key_count")? {
            index
                .included_columns
                .push(column.ok_or_else(|| sqlx::Error::ColumnNotFound("included column".into()))?);
            continue;
        }
        let target = match column {
            Some(name) => IndexTarget::Column(name),
            None => IndexTarget::Expression(Some(row.try_get("part_definition")?)),
        };
        let direction = match row.try_get::<Option<bool>, _>("descending")? {
            Some(true) => IndexDirection::Descending,
            _ => IndexDirection::Ascending,
        };
        index.parts.push(IndexPart {
            target,
            direction,
            prefix_length: None,
            collation: row.try_get("collation")?,
        });
    }
    Ok(indexes.into_values().collect())
}

fn index(row: &PgRow) -> Result<Index, sqlx::Error> {
    let origin = match (
        row.try_get::<bool, _>("indisprimary")?,
        row.try_get::<bool, _>("unique_constraint")?,
    ) {
        (true, _) => IndexOrigin::PrimaryKey,
        (_, true) => IndexOrigin::UniqueConstraint,
        _ => IndexOrigin::Explicit,
    };
    Ok(Index {
        name: row.try_get("name")?,
        origin,
        unique: row.try_get("indisunique")?,
        parts: Vec::new(),
        included_columns: Vec::new(),
        method: Some(row.try_get("method")?),
        predicate: row.try_get("predicate")?,
        native_definition: Some(row.try_get("definition")?),
        valid: row.try_get("valid")?,
    })
}
