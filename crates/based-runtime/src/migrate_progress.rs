//! `_based_migration_progress`: where an interrupted resumable apply stopped — the segment it
//! is on and, inside a chunked step, the last key written.

use based_codegen::Dialect;

use super::{placeholder, str_field};
use crate::run::{fetch_all, DbError, DbRead};
use crate::value::SqlValue;

const PROGRESS: &str = "_based_migration_progress";
const COLUMNS: [&str; 5] = ["id", "content_hash", "segment", "cursor", "rows_done"];

/// One saved position.
pub(super) struct Progress {
    pub hash: String,
    pub segment: usize,
    pub cursor: Option<Vec<String>>,
    pub rows: u64,
}

pub(super) fn create_progress_sql(dialect: Dialect) -> String {
    let (id_ty, hash_ty, int_ty, text_ty) = match dialect {
        Dialect::MariaDb | Dialect::MySql => ("VARCHAR(255)", "VARCHAR(64)", "BIGINT", "TEXT"),
        Dialect::Postgres => ("TEXT", "TEXT", "BIGINT", "TEXT"),
        Dialect::Sqlite => ("TEXT", "TEXT", "INTEGER", "TEXT"),
    };
    format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (\n  \
           {id} {id_ty} NOT NULL PRIMARY KEY,\n  \
           {hash} {hash_ty} NOT NULL,\n  \
           {seg} {int_ty} NOT NULL,\n  \
           {cur} {text_ty} NULL,\n  \
           {rows} {int_ty} NOT NULL\n)",
        tbl = dialect.quote(PROGRESS),
        id = dialect.quote("id"),
        hash = dialect.quote("content_hash"),
        seg = dialect.quote("segment"),
        cur = dialect.quote("cursor"),
        rows = dialect.quote("rows_done"),
    )
}

/// Record `key` at `segment` (and `cursor` inside a chunked step), replacing any earlier row.
pub(super) async fn save_progress<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    key: &str,
    hash: &str,
    segment: usize,
    cursor: Option<&[String]>,
    rows: u64,
) -> Result<(), DbError> {
    clear_progress(db, dialect, key).await?;
    let marks: Vec<String> = (1..=COLUMNS.len())
        .map(|n| placeholder(dialect, n))
        .collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        dialect.quote(PROGRESS),
        quoted(dialect, &COLUMNS),
        marks.join(", "),
    );
    let cursor = cursor.map_or(SqlValue::Null, |c| {
        SqlValue::Text(serde_json::to_string(c).unwrap_or_default())
    });
    db.execute(
        &sql,
        &[
            SqlValue::Text(key.to_string()),
            SqlValue::Text(hash.to_string()),
            SqlValue::Int(i64::try_from(segment).unwrap_or(i64::MAX)),
            cursor,
            SqlValue::Int(i64::try_from(rows).unwrap_or(i64::MAX)),
        ],
    )
    .await
    .map(|_| ())
}

pub(super) async fn clear_progress<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    key: &str,
) -> Result<(), DbError> {
    let sql = format!(
        "DELETE FROM {} WHERE {} = {}",
        dialect.quote(PROGRESS),
        dialect.quote("id"),
        placeholder(dialect, 1)
    );
    db.execute(&sql, &[SqlValue::Text(key.to_string())])
        .await
        .map(|_| ())
}

pub(super) async fn read_progress<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    key: &str,
) -> Result<Option<Progress>, DbError> {
    let sql = format!(
        "SELECT {} FROM {} WHERE {} = {}",
        quoted(dialect, &COLUMNS[1..]),
        dialect.quote(PROGRESS),
        dialect.quote("id"),
        placeholder(dialect, 1)
    );
    let rows = fetch_all(db.fetch(&sql, &[SqlValue::Text(key.to_string())])).await?;
    Ok(rows.first().map(|r| Progress {
        hash: str_field(r, "content_hash"),
        segment: usize::try_from(count(r, "segment")).unwrap_or(usize::MAX),
        cursor: r
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| serde_json::from_str(s).ok()),
        rows: count(r, "rows_done"),
    }))
}

fn quoted(dialect: Dialect, cols: &[&str]) -> String {
    cols.iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An integer column read back (drivers may carry it as a number or a string).
fn count(r: &crate::run::Row, key: &str) -> u64 {
    match r.get(key) {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_table_is_dialect_typed() {
        assert!(create_progress_sql(Dialect::Postgres).contains("\"cursor\" TEXT NULL"));
        assert!(create_progress_sql(Dialect::MariaDb).contains("`id` VARCHAR(255)"));
    }
}
