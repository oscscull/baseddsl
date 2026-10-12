//! Plan a `down.mig`: a `format 2` script — the reverse structural steps (derived from the
//! two snapshots), with `raw(<dialect>)` lines and data steps placed among them — or a legacy
//! raw-SQL file split on `;`.

use super::super::diff::strip_raw_steps;
use super::super::down_mig::{down_steps, render_down_steps, rollback_target};
use super::super::model::Snapshot;
use super::super::sql::content_hash;
use super::super::unit::PlanUnit;
use super::plan::{header, plan_in_order};
use super::scan::scan;
use super::split::split_sql;
use super::ScriptProblem;
use crate::Dialect;

const FILE: &str = "down.mig";

/// A migration's down path for one dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownPlan {
    pub units: Vec<PlanUnit>,
    pub review: String,
}

/// Plan `down_text`, the reverse of the migration from `prev` to `now`. Each data step is
/// checked against the schema state at its line, walking back from `now`. `Ok(None)` when
/// the file holds nothing executable (the migration stays roll-forward only).
pub fn plan_down(
    prev: &Snapshot,
    now: &Snapshot,
    down_text: &str,
    dialect: Dialect,
) -> Result<Option<DownPlan>, Vec<ScriptProblem>> {
    let scanned = scan(down_text);
    if !scanned.in_order {
        let units: Vec<PlanUnit> = split_sql(down_text)
            .into_iter()
            .map(PlanUnit::Sql)
            .collect();
        let review = format!("{}{down_text}", header(dialect));
        return Ok((!units.is_empty()).then_some(DownPlan { units, review }));
    }
    let back = rollback_target(prev, now);
    let steps = down_steps(prev, now);
    let expected = strip_raw_steps(&render_down_steps(&steps));
    if content_hash(&strip_raw_steps(down_text)) != content_hash(&expected) {
        return Err(vec![ScriptProblem {
            file: FILE,
            line: 0,
            message: "its structural lines diverge from the reverse of up.mig (derived from \
                      schema.snap) — restore them from `based migrate gen`, or use a \
                      raw(<dialect>) line"
                .to_string(),
        }]);
    }
    let plan = plan_in_order(FILE, now, &back, &steps, &scanned, dialect)?;
    Ok((!plan.units.is_empty()).then_some(DownPlan {
        units: plan.units,
        review: plan.review,
    }))
}
