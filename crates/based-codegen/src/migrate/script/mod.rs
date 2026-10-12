//! The authored `up.mig` as a script: its format marker, structural residue, `raw` escapes,
//! and data steps, planned into execution order.

mod place;
mod plan;
mod scan;

pub use plan::{plan_up, UpPlan, UpUnit};
pub use scan::FORMAT_MARKER;
pub(crate) use scan::{scan, ItemKind};

/// A problem with an `up.mig`, at a 1-based line (`0` = the migration as a whole).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpProblem {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for UpProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "up.mig line {}: {}", self.line, self.message)
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
