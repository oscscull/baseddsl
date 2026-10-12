//! The authored `up.mig` / `down.mig` as scripts: the format marker, structural residue,
//! `raw` escapes, and data steps, planned into execution order.

mod data_item;
mod down;
mod place;
mod plan;
mod scan;
mod split;

pub use down::{plan_down, DownPlan};
pub use plan::{plan_up, UpPlan};
pub use scan::FORMAT_MARKER;
pub(crate) use scan::{scan, ItemKind};

/// A problem in a migration script at a 1-based line (`0` = the migration as a whole).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptProblem {
    /// `up.mig` or `down.mig`.
    pub file: &'static str,
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ScriptProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}: {}", self.file, self.message)
        } else {
            write!(f, "{} line {}: {}", self.file, self.line, self.message)
        }
    }
}

/// Does an `up.mig` carry any typed data step?
pub fn has_data_step(up_text: &str) -> bool {
    scan(up_text)
        .items
        .iter()
        .any(|i| matches!(i.kind, ItemKind::Data(_)))
}
