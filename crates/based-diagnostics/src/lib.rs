//! based-diagnostics — diagnostic model shared across parser and sema.
//!
//! Stable codes (e.g. `E0001`, `W0100`) so lints can be referenced in the spec
//! and ratcheted warn -> error in CI (indexing.md, sorting.md). Rendering with
//! `ariadne` lands with the parser milestone.

use based_ast::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    /// Primary span; `None` for whole-project diagnostics (e.g. layout violations).
    pub span: Option<Span>,
    /// Secondary "note"/"help" lines.
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, severity: Severity::Error, message: message.into(), span: None, notes: Vec::new() }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, severity: Severity::Warning, message: message.into(), span: None, notes: Vec::new() }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}
