use based_catalog::{ForeignKey, MatchMode, ReferentialAction, TableId};
use sqlx::{postgres::PgConnection, Row};
use std::collections::BTreeMap;

pub(crate) async fn read(
    conn: &mut PgConnection,
    oid: i64,
) -> Result<Vec<ForeignKey>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/foreign_keys.sql")))
        .bind(oid)
        .fetch_all(conn)
        .await?;
    let mut keys = BTreeMap::<String, ForeignKey>::new();
    for row in rows {
        let name: String = row.try_get("name")?;
        let target = TableId::new(
            row.try_get::<String, _>("target_namespace")?,
            row.try_get::<String, _>("target_name")?,
        );
        let deferral = super::constraints::deferral(&row)?;
        let on_delete = action(&row.try_get::<String, _>("delete_action")?)?;
        let on_update = action(&row.try_get::<String, _>("update_action")?)?;
        let match_mode = match row.try_get::<String, _>("match_mode")?.as_str() {
            "s" => MatchMode::Simple,
            "f" => MatchMode::Full,
            "p" => MatchMode::Partial,
            _ => return Err(sqlx::Error::ColumnNotFound("foreign key match mode".into())),
        };
        let key = keys.entry(name.clone()).or_insert_with(|| ForeignKey {
            name: Some(name),
            columns: Vec::new(),
            target,
            target_columns: Vec::new(),
            on_delete,
            on_update,
            match_mode,
            deferral,
        });
        key.columns.push(row.try_get("column_name")?);
        key.target_columns.push(row.try_get("target_column")?);
    }
    Ok(keys.into_values().collect())
}

fn action(code: &str) -> Result<ReferentialAction, sqlx::Error> {
    match code {
        "a" => Ok(ReferentialAction::NoAction),
        "r" => Ok(ReferentialAction::Restrict),
        "c" => Ok(ReferentialAction::Cascade),
        "n" => Ok(ReferentialAction::SetNull),
        "d" => Ok(ReferentialAction::SetDefault),
        _ => Err(sqlx::Error::ColumnNotFound("foreign key action".into())),
    }
}
