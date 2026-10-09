//! Aggregate compiler diagnostics and count severity for callers.
use crate::{Error, Sources};
use based_diagnostics::{Diagnostic, Severity};

#[derive(Debug, Clone)]
pub struct Report {
    pub sources: Sources,
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }
    pub fn warnings(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }
    pub(super) fn ensure_clean(&self) -> Result<(), Error> {
        if self.errors() == 0 {
            return Ok(());
        }
        Err(Error::Diagnostics(self.clone()))
    }
}
