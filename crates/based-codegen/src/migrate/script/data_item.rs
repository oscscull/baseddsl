//! One data-step item of a script: parse, check against the schema state at its line, and
//! lower to a unit, with its review text.

use std::fmt::Write as _;

use based_ast::{DataStep, FileId};

use super::super::data::{check_step, lower_step, DataError};
use super::super::model::Snapshot;
use super::super::unit::PlanUnit;
use super::ScriptProblem;
use crate::Dialect;

/// The checked, lowered step starting at `line` of `file`.
pub(crate) fn data_unit(
    file: &'static str,
    line: usize,
    text: &str,
    snap: &Snapshot,
    dialect: Dialect,
) -> Result<(DataStep, PlanUnit), Vec<ScriptProblem>> {
    let at = |offset: usize, message: String| ScriptProblem {
        file,
        line: line + text[..offset.min(text.len())].matches('\n').count(),
        message,
    };
    let step = based_parser::parse_data_step(text, FileId(0)).map_err(|diags| {
        diags
            .into_iter()
            .map(|d| at(d.span.map_or(0, |s| s.start as usize), d.message))
            .collect::<Vec<_>>()
    })?;
    let from = |e: DataError| at(e.span.start as usize, e.message);
    let errs = check_step(&step, snap);
    if !errs.is_empty() {
        return Err(errs.into_iter().map(from).collect());
    }
    let unit = lower_step(&step, snap, dialect).map_err(|e| vec![from(e)])?;
    Ok((step, unit))
}

/// The review text of a lowered data step.
pub(crate) fn review(file: &str, line: usize, unit: &PlanUnit) -> String {
    let mut out = format!("\n-- data step ({file} line {line})\n");
    match unit {
        PlanUnit::Sql(sql) => {
            let _ = writeln!(out, "{sql};");
        }
        PlanUnit::Minted { sql, ids } => {
            let kinds: Vec<&str> = ids
                .iter()
                .map(|k| match k {
                    super::super::unit::IdKind::Uuid => "uuid",
                    super::super::unit::IdKind::Ulid => "ulid",
                })
                .collect();
            let _ = writeln!(out, "-- binds ids minted at apply: {}", kinds.join(", "));
            let _ = writeln!(out, "{sql};");
        }
        PlanUnit::Chunked(plan) => {
            let _ = writeln!(out, "{}", plan.review());
        }
    }
    out
}
