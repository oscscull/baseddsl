//! Resumable apply of a migration carrying chunked data steps. Each run of plain statements
//! and each batch of a chunked step commits on its own, together with a progress row in
//! `_based_migration_progress`, so an interrupted apply resumes where it stopped.

use based_codegen::migrate::{ChunkPlan, UpUnit};
use based_codegen::Dialect;

use super::failure::MigrationFailure;
use super::{insert_ledger_sql, placeholder, str_field, MigrateError, PlannedMigration};
use crate::run::{fetch_all, Backend, DbError, DbRead};
use crate::value::SqlValue;

const PROGRESS: &str = "_based_migration_progress";

/// One chunk's completion, for progress output.
#[derive(Debug, Clone, Copy)]
pub struct ChunkProgress<'a> {
    pub migration: &'a str,
    pub table: &'a str,
    /// Rows walked so far by this step.
    pub rows: u64,
    pub chunk: u64,
}

/// A run of plain statements, or one chunked step.
enum Segment<'a> {
    Sql(Vec<&'a str>),
    Chunk(&'a ChunkPlan),
}

fn segments(units: &[UpUnit]) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment<'_>> = Vec::new();
    for u in units {
        match (u, out.last_mut()) {
            (UpUnit::Sql(s), Some(Segment::Sql(run))) => run.push(s),
            (UpUnit::Sql(s), _) => out.push(Segment::Sql(vec![s])),
            (UpUnit::Chunked(p), _) => out.push(Segment::Chunk(p)),
        }
    }
    out
}

/// Where an interrupted apply stopped.
struct Progress {
    hash: String,
    segment: usize,
    cursor: Option<Vec<String>>,
    rows: u64,
}

/// Apply `m`, resuming from its recorded progress.
pub(super) async fn apply_resumable(
    backend: &dyn Backend,
    dialect: Dialect,
    m: &PlannedMigration,
    report: Option<fn(&ChunkProgress)>,
) -> Result<(), MigrateError> {
    let failed = |stage: String, source: DbError| {
        MigrateError::Execution(MigrationFailure {
            id: m.id.clone(),
            direction: "up",
            stage,
            dialect,
            source,
        })
    };
    let saved = {
        let mut db = backend.checkout("").await?;
        db.execute(&create_progress_sql(dialect), &[]).await?;
        read_progress(&mut *db, dialect, &m.id).await?
    };
    if let Some(p) = &saved {
        if p.hash != m.up_hash {
            return Err(MigrateError::Tamper {
                id: m.id.clone(),
                applied: p.hash.clone(),
                current: m.up_hash.clone(),
            });
        }
    }
    let segs = segments(&m.up);
    let start = saved.as_ref().map_or(0, |p| p.segment);
    for (i, seg) in segs.iter().enumerate().skip(start) {
        let last = i + 1 == segs.len();
        let resumed = saved.as_ref().filter(|p| p.segment == i);
        match seg {
            Segment::Sql(stmts) => {
                let db = backend.checkout("").await?;
                let mut tx = db.begin().await.map_err(|e| failed("begin".into(), e))?;
                for (n, s) in stmts.iter().enumerate() {
                    tx.execute(s, &[]).await.map_err(|e| {
                        failed(
                            format!("segment {} statement {} of {}", i + 1, n + 1, stmts.len()),
                            e,
                        )
                    })?;
                }
                finish_segment(&mut *tx, dialect, m, i, last)
                    .await
                    .map_err(|e| failed("progress update".into(), e))?;
                tx.commit().await.map_err(|e| failed("commit".into(), e))?;
            }
            Segment::Chunk(plan) => {
                let mut cursor = resumed.and_then(|p| p.cursor.clone());
                let mut rows = resumed.map_or(0, |p| p.rows);
                let mut chunk = 0u64;
                loop {
                    let db = backend.checkout("").await?;
                    let mut tx = db.begin().await.map_err(|e| failed("begin".into(), e))?;
                    let keys = fetch_all(tx.fetch(&plan.select_sql(cursor.as_deref()), &[]))
                        .await
                        .map_err(|e| failed(format!("{} batch select", plan.table), e))?;
                    let Some(upto) = keys.last().map(|row| key_of(plan, row)).transpose()? else {
                        finish_segment(&mut *tx, dialect, m, i, last)
                            .await
                            .map_err(|e| failed("progress update".into(), e))?;
                        tx.commit().await.map_err(|e| failed("commit".into(), e))?;
                        break;
                    };
                    tx.execute(&plan.apply_sql(cursor.as_deref(), &upto), &[])
                        .await
                        .map_err(|e| failed(format!("{} batch write", plan.table), e))?;
                    rows += keys.len() as u64;
                    chunk += 1;
                    save_progress(&mut *tx, dialect, m, i, Some(&upto), rows)
                        .await
                        .map_err(|e| failed("progress update".into(), e))?;
                    tx.commit().await.map_err(|e| failed("commit".into(), e))?;
                    if let Some(report) = report {
                        report(&ChunkProgress {
                            migration: &m.id,
                            table: &plan.table,
                            rows,
                            chunk,
                        });
                    }
                    cursor = Some(upto);
                    if let Some(ms) = plan.pause_ms {
                        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

/// A fetched batch row's walk-key values as SQL literals.
fn key_of(plan: &ChunkPlan, row: &crate::run::Row) -> Result<Vec<String>, MigrateError> {
    plan.key_columns
        .iter()
        .map(|k| {
            row.get(k).and_then(|v| plan.key_literal(v)).ok_or_else(|| {
                MigrateError::Artifact(format!(
                    "chunked step on `{}`: key `{k}` read back null or unsupported",
                    plan.table
                ))
            })
        })
        .collect()
}

/// Close segment `i`: the last one records the migration applied, any other moves the
/// progress to the next segment.
async fn finish_segment<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    m: &PlannedMigration,
    i: usize,
    last: bool,
) -> Result<(), DbError> {
    if last {
        clear_progress(db, dialect, &m.id).await?;
        db.execute(
            &insert_ledger_sql(dialect),
            &[
                SqlValue::Text(m.id.clone()),
                SqlValue::Text(m.up_hash.clone()),
            ],
        )
        .await
        .map(|_| ())
    } else {
        save_progress(db, dialect, m, i + 1, None, 0).await
    }
}

async fn save_progress<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    m: &PlannedMigration,
    segment: usize,
    cursor: Option<&[String]>,
    rows: u64,
) -> Result<(), DbError> {
    clear_progress(db, dialect, &m.id).await?;
    let sql = format!(
        "INSERT INTO {} ({}, {}, {}, {}, {}) VALUES ({}, {}, {}, {}, {})",
        dialect.quote(PROGRESS),
        dialect.quote("id"),
        dialect.quote("content_hash"),
        dialect.quote("segment"),
        dialect.quote("cursor"),
        dialect.quote("rows_done"),
        placeholder(dialect, 1),
        placeholder(dialect, 2),
        placeholder(dialect, 3),
        placeholder(dialect, 4),
        placeholder(dialect, 5),
    );
    let cursor = cursor.map_or(SqlValue::Null, |c| {
        SqlValue::Text(serde_json::to_string(c).unwrap_or_default())
    });
    db.execute(
        &sql,
        &[
            SqlValue::Text(m.id.clone()),
            SqlValue::Text(m.up_hash.clone()),
            SqlValue::Int(i64::try_from(segment).unwrap_or(i64::MAX)),
            cursor,
            SqlValue::Int(i64::try_from(rows).unwrap_or(i64::MAX)),
        ],
    )
    .await
    .map(|_| ())
}

async fn clear_progress<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    id: &str,
) -> Result<(), DbError> {
    let sql = format!(
        "DELETE FROM {} WHERE {} = {}",
        dialect.quote(PROGRESS),
        dialect.quote("id"),
        placeholder(dialect, 1)
    );
    db.execute(&sql, &[SqlValue::Text(id.to_string())])
        .await
        .map(|_| ())
}

async fn read_progress<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    id: &str,
) -> Result<Option<Progress>, DbError> {
    let sql = format!(
        "SELECT {}, {}, {}, {} FROM {} WHERE {} = {}",
        dialect.quote("content_hash"),
        dialect.quote("segment"),
        dialect.quote("cursor"),
        dialect.quote("rows_done"),
        dialect.quote(PROGRESS),
        dialect.quote("id"),
        placeholder(dialect, 1)
    );
    let rows = fetch_all(db.fetch(&sql, &[SqlValue::Text(id.to_string())])).await?;
    Ok(rows.first().map(|r| Progress {
        hash: str_field(r, "content_hash"),
        segment: count(r, "segment") as usize,
        cursor: r
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| serde_json::from_str(s).ok()),
        rows: count(r, "rows_done"),
    }))
}

/// An integer column read back (drivers may carry it as a number or a string).
fn count(r: &crate::run::Row, key: &str) -> u64 {
    match r.get(key) {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn create_progress_sql(dialect: Dialect) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_runs_group_between_chunked_steps() {
        let units = vec![
            UpUnit::Sql("a".into()),
            UpUnit::Sql("b".into()),
            UpUnit::Sql("c".into()),
        ];
        let segs = segments(&units);
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Sql(s) if s == &["a", "b", "c"]));
    }

    #[test]
    fn progress_table_is_dialect_typed() {
        assert!(create_progress_sql(Dialect::Postgres).contains("\"cursor\" TEXT NULL"));
        assert!(create_progress_sql(Dialect::MariaDb).contains("`id` VARCHAR(255)"));
    }
}
