//! Lower a checked data step to one executable unit for a dialect: a statement (binding ids
//! minted at apply time), or a [`ChunkPlan`] the runtime drives batch by batch.

use std::cell::Cell;

use based_ast::{DataOp, DataStep, Span};

use super::super::model::Snapshot;
use super::super::unit::PlanUnit;
use super::check::DataError;
use super::table::TableView;
use crate::Dialect;

mod chunk;
mod create;
mod expr;
mod json_path;
mod predicate;
mod write;

pub use chunk::ChunkPlan;

/// Lower `step` (already checked against `snap`) for `dialect`. `Err` for a construct the
/// dialect can't express.
pub(crate) fn lower_step(
    step: &DataStep,
    snap: &Snapshot,
    dialect: Dialect,
) -> Result<PlanUnit, DataError> {
    let table = step.op.table();
    let t = TableView::find(snap, &table.node).ok_or_else(|| DataError {
        span: table.span,
        message: format!("unknown table `{}`", table.node),
    })?;
    let lw = Lw {
        dialect,
        snap,
        root: t.name().to_string(),
        qualify: Some(t.name().to_string()),
        incoming: false,
        writes_root: !matches!(step.op, DataOp::Create { .. }),
        sub: Cell::new(0),
    };
    if let Some(chunk) = &step.chunk {
        return Ok(PlanUnit::Chunked(lw.chunk_plan(step, chunk, &t)?));
    }
    Ok(match &step.op {
        DataOp::Create { rows, conflict, .. } => {
            let (sql, ids) = lw.create(rows, conflict.as_ref(), &t)?;
            if ids.is_empty() {
                PlanUnit::Sql(sql)
            } else {
                PlanUnit::Minted { sql, ids }
            }
        }
        DataOp::Update {
            where_, assigns, ..
        } => PlanUnit::Sql(lw.update(where_.as_ref(), assigns, &t, None)?),
        DataOp::Delete { where_, .. } => PlanUnit::Sql(lw.delete(where_.as_ref(), &t, None)?),
    })
}

/// Lowering state for one step.
pub(crate) struct Lw<'a> {
    pub dialect: Dialect,
    pub snap: &'a Snapshot,
    /// The table the step writes.
    pub root: String,
    /// The alias column references are qualified with; `None` renders them bare.
    pub qualify: Option<String>,
    /// `incoming.<col>` names the proposed row of an upsert.
    pub incoming: bool,
    /// The step updates/deletes `root` (MySQL can't subquery the table it writes).
    pub writes_root: bool,
    /// Subquery alias counter.
    pub sub: Cell<usize>,
}

impl Lw<'_> {
    pub(crate) fn q(&self, ident: &str) -> String {
        self.dialect.quote(ident)
    }

    /// A column reference under the current qualifier.
    pub(crate) fn col_ref(&self, col: &str) -> String {
        match &self.qualify {
            Some(a) => self.dialect.qcol(a, col),
            None => self.q(col),
        }
    }

    pub(crate) fn qt(&self, t: &TableView) -> String {
        self.dialect.quote_table(t.snap.schema.as_deref(), t.name())
    }

    /// A lowering state reading another table under `alias`.
    pub(crate) fn nested(&self, alias: String) -> Lw<'_> {
        Lw {
            dialect: self.dialect,
            snap: self.snap,
            root: self.root.clone(),
            qualify: Some(alias),
            incoming: false,
            writes_root: self.writes_root,
            sub: Cell::new(self.sub.get()),
        }
    }

    pub(crate) fn fail(span: Span, message: impl Into<String>) -> DataError {
        DataError {
            span,
            message: message.into(),
        }
    }
}
