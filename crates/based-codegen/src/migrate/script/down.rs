//! Plan a `down.mig`: a `format 2` script of `raw(<dialect>)` lines and data steps run in
//! written order, or a legacy raw-SQL file split on `;`.

use super::super::model::Snapshot;
use super::super::sql::{render_sql, sql_statements};
use super::super::unit::PlanUnit;
use super::data_item::{data_unit, review};
use super::plan::{body, header};
use super::scan::{scan, ItemKind};
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

/// Plan `down_text` reversing the migration whose post-up state is `target`. Data steps are
/// checked against that state. `Ok(None)` when the file holds nothing executable (the
/// migration stays roll-forward only).
pub fn plan_down(
    target: &Snapshot,
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
    let mut problems: Vec<ScriptProblem> = down_text
        .lines()
        .enumerate()
        .filter(|(_, l)| scanned.structural.contains(l))
        .filter(|(_, l)| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .map(|(i, _)| ScriptProblem {
            file: FILE,
            line: i + 1,
            message: "a `format 2` down.mig holds data steps and raw(<dialect>) lines only"
                .to_string(),
        })
        .collect();
    let mut units = Vec::new();
    let mut text = header(dialect);
    for item in &scanned.items {
        match &item.kind {
            ItemKind::Raw(step) => {
                if let Ok(stmts) = sql_statements(std::slice::from_ref(&**step), dialect) {
                    units.extend(stmts.into_iter().map(PlanUnit::Sql));
                }
                text.push_str(body(&render_sql(std::slice::from_ref(&**step), dialect)));
            }
            ItemKind::Data(src) => match data_unit(FILE, item.line, src, target, dialect) {
                Ok((_, unit)) => {
                    text.push_str(&review(FILE, item.line, &unit));
                    units.push(unit);
                }
                Err(ps) => problems.extend(ps),
            },
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok((!units.is_empty()).then_some(DownPlan {
        units,
        review: text,
    }))
}
