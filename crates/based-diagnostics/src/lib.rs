//! based-diagnostics — diagnostic model shared across parser and sema.
//!
//! Stable codes (e.g. `E0001`, `W0100`) so lints can be referenced in the spec and
//! ratcheted warn -> error in CI.

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
    /// A one-key editor autofix: a member line to insert into a model's body. `None`
    /// when the diagnostic offers no mechanical fix.
    pub fix: Option<Fix>,
}

/// A mechanical fix an editor can apply in one keystroke: insert `line` as a member
/// of model `model`'s body. Used by `E0260` (insert an `@index`) and `E0261`
/// (insert the `id` line).
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    /// The model whose body gains the line.
    pub model: String,
    /// The member line to insert (e.g. `@index order`, `id: Id`).
    pub line: String,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span: None,
            notes: Vec::new(),
            fix: None,
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            span: None,
            notes: Vec::new(),
            fix: None,
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Attach a one-key autofix: insert `line` into model `model`'s body.
    pub fn with_fix(mut self, model: impl Into<String>, line: impl Into<String>) -> Self {
        self.fix = Some(Fix {
            model: model.into(),
            line: line.into(),
        });
        self
    }
}
