//! Turn a migration script + snapshot pair into its executable units and review text for a
//! dialect: structural SQL, `raw` escapes, and checked data steps, in execution order.

use super::super::data::invert;
use super::super::diff::{diff_snapshots, Step};
use super::super::evolve::evolve;
use super::super::model::Snapshot;
use super::super::sql::{migration_sql, render_migration, render_sql, sql_statements};
use super::super::step_order::constraint_safe;
use super::super::unit::PlanUnit;
use super::data_item::{data_unit, review};
use super::place::{place, Placed};
use super::scan::{scan, ItemKind, FORMAT_MARKER};
use super::ScriptProblem;
use crate::Dialect;

const UP: &str = "up.mig";

/// A migration's up path for one dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpPlan {
    pub units: Vec<PlanUnit>,
    /// The SQL a reviewer reads (`based migrate render`).
    pub review: String,
    /// The automatic down path, when every step is a reversible literal-row `create`.
    pub auto_down: Option<Vec<PlanUnit>>,
    /// The migration carries typed data steps.
    pub has_data: bool,
}

fn problem(file: &'static str, line: usize, message: String) -> ScriptProblem {
    ScriptProblem {
        file,
        line,
        message,
    }
}

/// Plan the migration from `prev` to `target` whose authored text is `up_text`. Structural
/// drift is checked separately ([`super::super::up_mig_matches_snapshot`]).
pub fn plan_up(
    prev: &Snapshot,
    target: &Snapshot,
    up_text: &str,
    dialect: Dialect,
) -> Result<UpPlan, Vec<ScriptProblem>> {
    let mut steps = diff_snapshots(prev, target);
    let scanned = scan(up_text);
    if !scanned.in_order {
        let first_data = scanned
            .items
            .iter()
            .find(|i| matches!(i.kind, ItemKind::Data(_)));
        return match first_data {
            Some(item) => Err(vec![problem(
                UP,
                item.line,
                format!(
                    "data steps run in written order — add `{FORMAT_MARKER}` as the first line \
                     (raw lines then run where they are written too)"
                ),
            )]),
            None => legacy_plan(&steps, up_text, dialect, target),
        };
    }
    constraint_safe(&mut steps);
    plan_in_order(UP, prev, target, &steps, &scanned, dialect)
}

/// Plan an in-order script running `steps` from `from` to `to`, its `raw` lines and data
/// steps placed among them; each data step is checked against the state at its line.
pub(crate) fn plan_in_order(
    file: &'static str,
    from: &Snapshot,
    to: &Snapshot,
    steps: &[Step],
    scanned: &super::scan::Scanned<'_>,
    dialect: Dialect,
) -> Result<UpPlan, Vec<ScriptProblem>> {
    let placed = place(file, steps, scanned).map_err(|p| vec![p])?;
    let mut walk = Walk {
        file,
        dialect,
        snap: from.clone(),
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
    walk.snap = to.clone();
    walk.flush();
    if !walk.problems.is_empty() {
        return Err(walk.problems);
    }
    let auto_down = walk
        .downs
        .filter(|_| walk.has_data)
        .map(|d| d.into_iter().rev().map(PlanUnit::Sql).collect());
    Ok(UpPlan {
        units: walk.units,
        review: walk.review,
        auto_down,
        has_data: walk.has_data,
    })
}

/// The legacy order: every structural step, then every `raw` line.
fn legacy_plan(
    steps: &[Step],
    up_text: &str,
    dialect: Dialect,
    target: &Snapshot,
) -> Result<UpPlan, Vec<ScriptProblem>> {
    let whole = |message: String| vec![problem(UP, 0, message)];
    let raws = super::super::diff::parse_raw_steps(up_text);
    let units = migration_sql(steps, dialect, target)
        .map_err(whole)?
        .into_iter()
        .chain(sql_statements(&raws, dialect).map_err(whole)?)
        .map(PlanUnit::Sql)
        .collect();
    let all: Vec<Step> = steps.iter().cloned().chain(raws).collect();
    Ok(UpPlan {
        units,
        review: render_migration(&all, dialect, target),
        auto_down: None,
        has_data: false,
    })
}

pub(crate) fn header(dialect: Dialect) -> String {
    format!(
        "-- Rendered by `based migrate render` (dialect: {}). Review before apply.\n",
        dialect.name()
    )
}

/// Drop the leading "Rendered by" line of a rendered segment.
pub(crate) fn body(rendered: &str) -> &str {
    rendered.split_once('\n').map_or("", |(_, rest)| rest)
}

struct Walk {
    file: &'static str,
    dialect: Dialect,
    /// The schema state at the current line.
    snap: Snapshot,
    /// Structural steps since the last raw/data item.
    pending: Vec<Step>,
    units: Vec<PlanUnit>,
    review: String,
    problems: Vec<ScriptProblem>,
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
            Ok(stmts) => self.units.extend(stmts.into_iter().map(PlanUnit::Sql)),
            Err(message) => self.problems.push(problem(self.file, 0, message)),
        }
        self.review
            .push_str(body(&render_migration(&steps, self.dialect, &self.snap)));
    }

    fn raw(&mut self, step: &Step) {
        let stmts = sql_statements(std::slice::from_ref(step), self.dialect);
        self.units
            .extend(stmts.into_iter().flatten().map(PlanUnit::Sql));
        self.review
            .push_str(body(&render_sql(std::slice::from_ref(step), self.dialect)));
    }

    fn data(&mut self, line: usize, text: &str) {
        self.has_data = true;
        match data_unit(self.file, line, text, &self.snap, self.dialect) {
            Ok((step, unit)) => {
                self.review.push_str(&review(self.file, line, &unit));
                self.units.push(unit);
                let inverse = invert(&step, &self.snap, self.dialect);
                match (&mut self.downs, inverse) {
                    (Some(d), Some(sql)) => d.push(sql),
                    (d, _) => *d = None,
                }
            }
            Err(problems) => {
                self.downs = None;
                self.problems.extend(problems);
            }
        }
    }
}
