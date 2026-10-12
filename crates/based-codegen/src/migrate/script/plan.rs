//! Turn one migration's `up.mig` + snapshot pair into its executable units and review text
//! for a dialect: structural SQL, `raw` escapes, and checked data steps, in execution order.

use std::fmt::Write as _;

use based_ast::FileId;

use super::super::data::{check_step, invert, lower_step, ChunkPlan, DataError, Lowered};
use super::super::diff::{diff_snapshots, Step};
use super::super::evolve::evolve;
use super::super::model::Snapshot;
use super::super::sql::{migration_sql, render_migration, render_sql, sql_statements};
use super::place::{place, Placed};
use super::scan::{scan, ItemKind, FORMAT_MARKER};
use super::UpProblem;
use crate::Dialect;

/// One executable unit of a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpUnit {
    Sql(String),
    /// A chunked data step, run batch by batch.
    Chunked(ChunkPlan),
}

/// A migration's up path for one dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpPlan {
    pub units: Vec<UpUnit>,
    /// The SQL a reviewer reads (`based migrate render`).
    pub review: String,
    /// The automatic down path, when every step is a reversible literal-row `create`.
    pub auto_down: Option<Vec<String>>,
    /// The migration carries typed data steps.
    pub has_data: bool,
}

/// Plan the migration from `prev` to `target` whose authored text is `up_text`. Structural
/// drift is checked separately ([`super::super::up_mig_matches_snapshot`]).
pub fn plan_up(
    prev: &Snapshot,
    target: &Snapshot,
    up_text: &str,
    dialect: Dialect,
) -> Result<UpPlan, Vec<UpProblem>> {
    let steps = diff_snapshots(prev, target);
    let scanned = scan(up_text);
    if !scanned.in_order {
        if let Some(item) = scanned
            .items
            .iter()
            .find(|i| matches!(i.kind, ItemKind::Data(_)))
        {
            return Err(vec![UpProblem {
                line: item.line,
                message: format!(
                    "data steps run in written order — add `{FORMAT_MARKER}` as the first line \
                     (raw lines then run where they are written too)"
                ),
            }]);
        }
        return legacy_plan(&steps, up_text, dialect, target);
    }
    let placed = place(&steps, &scanned).map_err(|p| vec![p])?;
    let mut walk = Walk {
        dialect,
        snap: prev.clone(),
        pending: Vec::new(),
        units: Vec::new(),
        review: header(dialect),
        problems: Vec::new(),
        downs: Some(Vec::new()),
        has_data: false,
    };
    for entry in &placed {
        match entry {
            Placed::Step(step) => {
                if !matches!(step, Step::ScopeChange(_)) {
                    walk.downs = None;
                }
                evolve(&mut walk.snap, step);
                walk.pending.push((*step).clone());
            }
            Placed::Raw(step) => {
                walk.downs = None;
                walk.flush();
                walk.raw(step);
            }
            Placed::Data { line, text } => {
                walk.flush();
                walk.data(*line, text);
            }
        }
    }
    // Every step has applied: the remaining segment targets the recorded snapshot itself.
    walk.snap = target.clone();
    walk.flush();
    if !walk.problems.is_empty() {
        return Err(walk.problems);
    }
    Ok(UpPlan {
        units: walk.units,
        review: walk.review,
        auto_down: walk.downs.filter(|_| walk.has_data).map(|mut d| {
            d.reverse();
            d
        }),
        has_data: walk.has_data,
    })
}

/// The legacy order: every structural step, then every `raw` line.
fn legacy_plan(
    steps: &[Step],
    up_text: &str,
    dialect: Dialect,
    target: &Snapshot,
) -> Result<UpPlan, Vec<UpProblem>> {
    let problem = |message: String| vec![UpProblem { line: 0, message }];
    let raws = super::super::diff::parse_raw_steps(up_text);
    let mut units: Vec<UpUnit> = migration_sql(steps, dialect, target)
        .map_err(problem)?
        .into_iter()
        .map(UpUnit::Sql)
        .collect();
    units.extend(
        sql_statements(&raws, dialect)
            .map_err(problem)?
            .into_iter()
            .map(UpUnit::Sql),
    );
    let mut all = steps.to_vec();
    all.extend(raws);
    Ok(UpPlan {
        units,
        review: render_migration(&all, dialect, target),
        auto_down: None,
        has_data: false,
    })
}

fn header(dialect: Dialect) -> String {
    format!(
        "-- Rendered by `based migrate render` (dialect: {}). Review before apply.\n",
        dialect.name()
    )
}

/// Drop the leading "Rendered by" line of a rendered segment.
fn body(rendered: &str) -> &str {
    rendered.split_once('\n').map_or("", |(_, rest)| rest)
}

struct Walk {
    dialect: Dialect,
    /// The schema state at the current line.
    snap: Snapshot,
    /// Structural steps since the last raw/data item.
    pending: Vec<Step>,
    units: Vec<UpUnit>,
    review: String,
    problems: Vec<UpProblem>,
    /// Reverse statements so far; `None` once anything irreversible is seen.
    downs: Option<Vec<String>>,
    has_data: bool,
}

impl Walk {
    /// Emit the pending structural steps, SQLite rebuilds targeting the current state.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let steps = std::mem::take(&mut self.pending);
        match migration_sql(&steps, self.dialect, &self.snap) {
            Ok(stmts) => self.units.extend(stmts.into_iter().map(UpUnit::Sql)),
            Err(message) => self.problems.push(UpProblem { line: 0, message }),
        }
        self.review
            .push_str(body(&render_migration(&steps, self.dialect, &self.snap)));
    }

    fn raw(&mut self, step: &Step) {
        if let Ok(stmts) = sql_statements(std::slice::from_ref(step), self.dialect) {
            self.units.extend(stmts.into_iter().map(UpUnit::Sql));
        }
        self.review
            .push_str(body(&render_sql(std::slice::from_ref(step), self.dialect)));
    }

    fn data(&mut self, line: usize, text: &str) {
        self.has_data = true;
        let at = |e: &DataError| UpProblem {
            line: line
                + text[..(e.span.start as usize).min(text.len())]
                    .matches('\n')
                    .count(),
            message: e.message.clone(),
        };
        let step = match based_parser::parse_data_step(text, FileId(0)) {
            Ok(s) => s,
            Err(diags) => {
                self.downs = None;
                for d in diags {
                    let start = d.span.map_or(0, |s| s.start as usize).min(text.len());
                    self.problems.push(UpProblem {
                        line: line + text[..start].matches('\n').count(),
                        message: d.message,
                    });
                }
                return;
            }
        };
        let errs = check_step(&step, &self.snap);
        if !errs.is_empty() {
            self.downs = None;
            self.problems.extend(errs.iter().map(at));
            return;
        }
        match lower_step(&step, &self.snap, self.dialect) {
            Ok(Lowered::Sql(stmts)) => {
                let _ = writeln!(self.review, "\n-- data step (up.mig line {line})");
                for s in &stmts {
                    let _ = writeln!(self.review, "{s};");
                }
                self.units.extend(stmts.into_iter().map(UpUnit::Sql));
            }
            Ok(Lowered::Chunked(plan)) => {
                let _ = writeln!(self.review, "\n-- data step (up.mig line {line})");
                let _ = writeln!(self.review, "{}", plan.review());
                self.units.push(UpUnit::Chunked(plan));
            }
            Err(e) => self.problems.push(at(&e)),
        }
        let inverse = invert(&step, &self.snap, self.dialect);
        match (&mut self.downs, inverse) {
            (Some(d), Some(sql)) => d.push(sql),
            (d, _) => *d = None,
        }
    }
}
