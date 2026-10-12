//! Resumable run of a migration direction carrying chunked data steps. Each run of plain
//! statements and each batch of a chunked step commits on its own, together with a progress
//! row, so an interrupted apply (or rollback) resumes where it stopped.

use based_codegen::migrate::{ChunkPlan, PlanUnit};
use based_codegen::Dialect;

use super::bind::bind;
use super::failure::MigrationFailure;
use super::progress::{clear_progress, create_progress_sql, read_progress, save_progress};
use super::{delete_ledger_sql, insert_ledger_sql, ApplyOpts, MigrateError};
use crate::run::{fetch_all, Backend, DbError, DbRead, Tx};
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
    Sql(&'a [PlanUnit]),
    Chunk(&'a ChunkPlan),
}

fn segments(units: &[PlanUnit]) -> Vec<Segment<'_>> {
    let chunked = |u: &PlanUnit| matches!(u, PlanUnit::Chunked(_));
    units
        .chunk_by(|a, b| !chunked(a) && !chunked(b))
        .map(|run| match run {
            [PlanUnit::Chunked(plan)] => Segment::Chunk(plan),
            _ => Segment::Sql(run),
        })
        .collect()
}

/// Run `run`, resuming from its recorded progress.
pub(super) async fn run_resumable(
    backend: &dyn Backend,
    dialect: Dialect,
    run: &Run<'_>,
    opts: &ApplyOpts,
) -> Result<(), MigrateError> {
    let r = Resume {
        backend,
        dialect,
        run,
        opts,
        key: run.key(),
    };
    let saved = {
        let mut db = backend.checkout("").await?;
        db.execute(&create_progress_sql(dialect), &[]).await?;
        read_progress(&mut *db, dialect, &r.key).await?
    };
    if let Some(p) = saved.as_ref().filter(|p| p.hash != run.hash) {
        return Err(MigrateError::Tamper {
            id: run.id.to_string(),
            applied: p.hash.clone(),
            current: run.hash.to_string(),
        });
    }
    let segs = segments(run.units);
    let last = segs.len().saturating_sub(1);
    for (i, seg) in segs
        .iter()
        .enumerate()
        .skip(saved.as_ref().map_or(0, |p| p.segment))
    {
        let at = Position { i, last: i == last };
        match seg {
            Segment::Sql(units) => r.statements(units, at).await?,
            Segment::Chunk(plan) => {
                let resumed = saved.as_ref().filter(|p| p.segment == i);
                let cursor = resumed.and_then(|p| p.cursor.clone());
                r.chunks(plan, at, cursor, resumed.map_or(0, |p| p.rows))
                    .await?;
            }
        }
    }
    Ok(())
}

/// A segment's index, and whether it is the run's last.
#[derive(Clone, Copy)]
struct Position {
    i: usize,
    last: bool,
}

struct Resume<'a> {
    backend: &'a dyn Backend,
    dialect: Dialect,
    run: &'a Run<'a>,
    opts: &'a ApplyOpts,
    key: String,
}

impl Resume<'_> {
    fn failed(&self, stage: impl Into<String>) -> impl FnOnce(DbError) -> MigrateError + '_ {
        let stage = stage.into();
        move |source| {
            MigrateError::Execution(MigrationFailure {
                id: self.run.id.to_string(),
                direction: if self.run.down { "down" } else { "up" },
                stage,
                dialect: self.dialect,
                source,
            })
        }
    }

    async fn begin(&self) -> Result<Box<dyn Tx>, MigrateError> {
        let db = self.backend.checkout("").await?;
        db.begin().await.map_err(self.failed("begin"))
    }

    async fn commit(&self, tx: Box<dyn Tx>) -> Result<(), MigrateError> {
        tx.commit().await.map_err(self.failed("commit"))
    }

    /// Close the segment at `at` and commit `tx`.
    async fn finish(&self, mut tx: Box<dyn Tx>, at: Position) -> Result<(), MigrateError> {
        finish_segment(&mut *tx, self.dialect, self.run, at.i, at.last)
            .await
            .map_err(self.failed("progress update"))?;
        self.commit(tx).await
    }

    /// Plain statements, in one transaction with the segment's close.
    async fn statements(&self, units: &[PlanUnit], at: Position) -> Result<(), MigrateError> {
        let mut tx = self.begin().await?;
        for (n, u) in units.iter().enumerate() {
            let b = bind(u, self.run.id, self.opts.mint)?;
            tx.execute(&b.sql, &b.params)
                .await
                .map_err(self.failed(format!(
                    "segment {} statement {} of {}",
                    at.i + 1,
                    n + 1,
                    units.len()
                )))?;
        }
        self.finish(tx, at).await
    }

    /// A chunked step from `cursor`, one transaction per batch, until a batch comes back empty.
    async fn chunks(
        &self,
        plan: &ChunkPlan,
        at: Position,
        mut cursor: Option<Vec<String>>,
        mut rows: u64,
    ) -> Result<(), MigrateError> {
        let mut chunk = 0u64;
        loop {
            chunk += 1;
            let mut tx = self.begin().await?;
            let keys = fetch_all(tx.fetch(&plan.select_sql(cursor.as_deref()), &[]))
                .await
                .map_err(self.failed(format!("{} batch select", plan.table)))?;
            let Some(upto) = keys.last().map(|row| key_of(plan, row)).transpose()? else {
                return self.finish(tx, at).await;
            };
            tx.execute(&plan.apply_sql(cursor.as_deref(), &upto), &[])
                .await
                .map_err(self.failed(format!("{} batch write", plan.table)))?;
            rows += keys.len() as u64;
            save_progress(
                &mut *tx,
                self.dialect,
                &self.key,
                self.run.hash,
                at.i,
                Some(&upto),
                rows,
            )
            .await
            .map_err(self.failed("progress update"))?;
            self.commit(tx).await?;
            if let Some(report) = self.opts.progress {
                report(&ChunkProgress {
                    migration: self.run.id,
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
