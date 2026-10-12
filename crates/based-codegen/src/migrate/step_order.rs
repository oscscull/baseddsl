//! The step order of a `format 2` script: foreign keys and indexes come off before the
//! columns and tables they cover, and go back on after, so no dialect rejects a drop of a
//! still-referenced column. Legacy scripts keep the diff engine's order.

use super::diff::{diff_snapshots, Step};
use super::model::Snapshot;
use super::script::scan;

/// Reorder `steps` for a `format 2` script (stable within each phase).
pub(crate) fn constraint_safe(steps: &mut [Step]) {
    steps.sort_by_key(phase);
}

fn phase(step: &Step) -> u8 {
    match step {
        Step::DropForeignKey { .. } => 0,
        Step::DropIndex { .. } | Step::DropUnique { .. } => 1,
        Step::AddIndex { .. } | Step::AddUnique { .. } => 3,
        Step::AddForeignKey { .. } => 4,
        _ => 2,
    }
}

/// The structural steps of the migration from `prev` to `now`, in the order its stored
/// `up_text` runs them (by its format).
pub fn up_steps(prev: &Snapshot, now: &Snapshot, up_text: &str) -> Vec<Step> {
    let mut steps = diff_snapshots(prev, now);
    if scan(up_text).in_order {
        constraint_safe(&mut steps);
    }
    steps
}
