use based_catalog::{Deferral, ForeignKey, MatchMode, ReferentialAction, TableId};
use sqlx::{Row, SqliteConnection};
use std::collections::BTreeMap;

pub(crate) async fn read(
    conn: &mut SqliteConnection,
    name: &str,
    unknown_timing: bool,
) -> Result<Vec<ForeignKey>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!("sql/foreign_keys.sql")))
        .bind(name)
        .fetch_all(&mut *conn)
        .await?;
    let mut keys = BTreeMap::<i64, ForeignKey>::new();
    for row in rows {
        let ordinal: i64 = row.try_get("id")?;
        let target: String = row.try_get("target")?;
        let on_update = action(&row.try_get::<String, _>("on_update")?)?;
        let on_delete = action(&row.try_get::<String, _>("on_delete")?)?;
        let deferral = match unknown_timing {
            true => Deferral::Unknown,
            false => Deferral::NotDeferrable,
        };
        let foreign = keys.entry(ordinal).or_insert_with(|| ForeignKey {
            name: None,
            columns: Vec::new(),
            target: TableId::new("main", target),
            target_columns: Vec::new(),
            on_delete,
            on_update,
            match_mode: MatchMode::Simple,
            deferral,
        });
        foreign.columns.push(row.try_get("local_column")?);
        // A missing target column means REFERENCES table's PK shorthand. Do not guess it.
        if let Some(column) = row.try_get::<Option<String>, _>("target_column")? {
            foreign.target_columns.push(column);
        }
    }
    let mut normalized = Vec::new();
    for mut foreign in keys.into_values() {
        if foreign.target_columns.is_empty() {
            // REFERENCES table with no column list means its declared PK, not a name heuristic.
            foreign.target_columns = sqlx::query_scalar(sqlx::AssertSqlSafe(
                "SELECT name FROM pragma_table_xinfo(?, 'main') WHERE pk > 0 ORDER BY pk",
            ))
            .bind(&foreign.target.name)
            .fetch_all(&mut *conn)
            .await?;
        }
        normalized.push(foreign);
    }
    Ok(normalized)
}

fn action(rule: &str) -> Result<ReferentialAction, sqlx::Error> {
    match rule {
        "NO ACTION" => Ok(ReferentialAction::NoAction),
        "RESTRICT" => Ok(ReferentialAction::Restrict),
        "CASCADE" => Ok(ReferentialAction::Cascade),
        "SET NULL" => Ok(ReferentialAction::SetNull),
        "SET DEFAULT" => Ok(ReferentialAction::SetDefault),
        _ => Err(sqlx::Error::ColumnNotFound("referential action".into())),
    }
}
