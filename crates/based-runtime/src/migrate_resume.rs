//! Resumable run of a migration direction carrying chunked data steps. Each run of plain
//! statements and each batch of a chunked step commits on its own, together with a progress
//! row, so an interrupted apply (or rollback) resumes where it stopped.

use based_codegen::migrate::{ChunkPlan, PlanUnit};
use based_codegen::Dialect;

use super::bind::bind;
use super::failure::MigrationFailure;
use super::progress::{clear_progress, create_progress_sql, read_progress, save_progress};
use super::{delete_ledger_sql, insert_ledger_sql, ApplyOpts, MigrateError};
use crate::run::{fetch_all, Backend, DbError, DbRead};
use crate::value::SqlValue;

/// One chunk's completion, for progress output.
#[derive(Debug, Clone, Copy)]
pub struct ChunkProgress<'a> {
    pub migration: &'a str,
    pub table: &'a str,
    /// Rows walked so far by this step.
    pub rows: u64,
    pub chunk: u64,
}

/// One direction of one migration.
pub(super) struct Run<'a> {
    pub id: &'a str,
    pub hash: &'a str,
    pub down: bool,
    pub units: &'a [PlanUnit],
}

impl Run<'_> {
    /// The progress row's key — up and down runs of a migration resume independently.
    fn key(&self) -> String {
        if self.down {
            format!("{}:down", self.id)
        } else {
            self.id.to_string()
        }
    }
}

/// A run of plain statements, or one chunked step.
enum Segment<'a> {
    Sql(Vec<&'a PlanUnit>),
    Chunk(&'a ChunkPlan),
}

fn segments(units: &[PlanUnit]) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment<'_>> = Vec::new();
    for u in units {
        match (u, out.last_mut()) {
            (PlanUnit::Chunked(p), _) => out.push(Segment::Chunk(p)),
            (_, Some(Segment::Sql(run))) => run.push(u),
            (_, _) => out.push(Segment::Sql(vec![u])),
        }
    }
    out
}

/// Run `run`, resuming from its recorded progress.
pub(super) async fn run_resumable(
    backend: &dyn Backend,
    dialect: Dialect,
    run: &Run<'_>,
    opts: &ApplyOpts,
) -> Result<(), MigrateError> {
    let failed = |stage: String, source: DbError| {
        MigrateError::Execution(MigrationFailure {
            id: run.id.to_string(),
            direction: if run.down { "down" } else { "up" },
            stage,
            dialect,
            source,
        })
    };
    let key = run.key();
    let saved = {
        let mut db = backend.checkout("").await?;
        db.execute(&create_progress_sql(dialect), &[]).await?;
        read_progress(&mut *db, dialect, &key).await?
    };
    if let Some(p) = saved.as_ref().filter(|p| p.hash != run.hash) {
        return Err(MigrateError::Tamper {
            id: run.id.to_string(),
            applied: p.hash.clone(),
            current: run.hash.to_string(),
        });
    }
    let segs = segments(run.units);
    let start = saved.as_ref().map_or(0, |p| p.segment);
    for (i, seg) in segs.iter().enumerate().skip(start) {
        let last = i + 1 == segs.len();
        match seg {
            Segment::Sql(units) => {
                let db = backend.checkout("").await?;
                let mut tx = db.begin().await.map_err(|e| failed("begin".into(), e))?;
                for (n, u) in units.iter().enumerate() {
                    let b = bind(u, run.id, opts.mint)?;
                    tx.execute(&b.sql, &b.params).await.map_err(|e| {
                        failed(
                            format!("segment {} statement {} of {}", i + 1, n + 1, units.len()),
                            e,
                        )
                    })?;
                }
                finish_segment(&mut *tx, dialect, run, i, last)
                    .await
                    .map_err(|e| failed("progress update".into(), e))?;
                tx.commit().await.map_err(|e| failed("commit".into(), e))?;
            }
            Segment::Chunk(plan) => {
                let resumed = saved.as_ref().filter(|p| p.segment == i);
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
                        finish_segment(&mut *tx, dialect, run, i, last)
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
                    save_progress(&mut *tx, dialect, &key, run.hash, i, Some(&upto), rows)
                        .await
                        .map_err(|e| failed("progress update".into(), e))?;
                    tx.commit().await.map_err(|e| failed("commit".into(), e))?;
                    if let Some(report) = opts.progress {
                        report(&ChunkProgress {
                            migration: run.id,
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

/// Close segment `i`: the last one updates the ledger (recording the migration applied, or
/// removing it on rollback), any other moves the progress to the next segment.
async fn finish_segment<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
    run: &Run<'_>,
    i: usize,
    last: bool,
) -> Result<(), DbError> {
    let key = run.key();
    if !last {
        return save_progress(db, dialect, &key, run.hash, i + 1, None, 0).await;
    }
    clear_progress(db, dialect, &key).await?;
    let (sql, params) = if run.down {
        (
            delete_ledger_sql(dialect),
            vec![SqlValue::Text(run.id.to_string())],
        )
    } else {
        (
            insert_ledger_sql(dialect),
            vec![
                SqlValue::Text(run.id.to_string()),
                SqlValue::Text(run.hash.to_string()),
            ],
        )
    };
    db.execute(&sql, &params).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_runs_group_between_chunked_steps() {
        let units = vec![
            PlanUnit::Sql("a".into()),
            PlanUnit::Sql("b".into()),
            PlanUnit::Sql("c".into()),
        ];
        let segs = segments(&units);
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], Segment::Sql(s) if s.len() == 3));
    }
}
