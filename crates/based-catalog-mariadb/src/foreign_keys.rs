use based_catalog::{Deferral, ForeignKey, MatchMode, ReferentialAction, TableId};
use sqlx::{mysql::MySqlConnection, Row};
use std::collections::BTreeMap;

pub(crate) async fn read(
    conn: &mut MySqlConnection,
    id: &TableId,
) -> Result<Vec<ForeignKey>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/foreign_keys.sql")))
        .bind(&id.namespace)
        .bind(&id.name)
        .fetch_all(conn)
        .await?;
    let mut keys = BTreeMap::<String, ForeignKey>::new();
    for row in rows {
        let name: String = row.try_get("CONSTRAINT_NAME")?;
        let target = TableId::new(
            row.try_get::<String, _>("REFERENCED_TABLE_SCHEMA")?,
            row.try_get::<String, _>("REFERENCED_TABLE_NAME")?,
        );
        let on_delete = action(&row.try_get::<String, _>("DELETE_RULE")?)?;
        let on_update = action(&row.try_get::<String, _>("UPDATE_RULE")?)?;
        let key = keys.entry(name.clone()).or_insert_with(|| ForeignKey {
            name: Some(name),
            columns: Vec::new(),
            target,
            target_columns: Vec::new(),
            on_delete,
            on_update,
            match_mode: MatchMode::Simple,
            deferral: Deferral::NotDeferrable,
        });
        key.columns.push(row.try_get("COLUMN_NAME")?);
        key.target_columns
            .push(row.try_get("REFERENCED_COLUMN_NAME")?);
    }
    Ok(keys.into_values().collect())
}

fn action(rule: &str) -> Result<ReferentialAction, sqlx::Error> {
    match rule {
        "NO ACTION" => Ok(ReferentialAction::NoAction),
        "RESTRICT" => Ok(ReferentialAction::Restrict),
        "CASCADE" => Ok(ReferentialAction::Cascade),
        "SET NULL" => Ok(ReferentialAction::SetNull),
        "SET DEFAULT" => Ok(ReferentialAction::SetDefault),
        _ => Err(sqlx::Error::ColumnNotFound(
            "unknown referential action".into(),
        )),
    }
}
