//! Offline check of one data step against the schema state at its line: names, types,
//! writability, required columns, conflict keys, and the `chunk` modifier's walk key.

use based_ast::{DataStep, Span};

use super::super::model::Snapshot;

mod chunk;
mod expr;
mod ops;
pub(crate) mod predicate;

/// One check failure, anchored in the step's own text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataError {
    pub span: Span,
    pub message: String,
}

/// Every problem with `step` against `snap`.
pub(crate) fn check_step(step: &DataStep, snap: &Snapshot) -> Vec<DataError> {
    let mut ck = Ck {
        snap,
        errs: Vec::new(),
    };
    ck.step(step);
    ck.errs
}

pub(crate) struct Ck<'a> {
    pub snap: &'a Snapshot,
    pub errs: Vec<DataError>,
}

impl Ck<'_> {
    pub(crate) fn err(&mut self, span: Span, message: impl Into<String>) {
        self.errs.push(DataError {
            span,
            message: message.into(),
        });
    }
}

/// What a value position may read besides literals.
#[derive(Clone, Copy)]
pub(crate) struct Scope {
    /// Same-row columns (an update, a `where`, an upsert's existing row).
    pub columns: bool,
    /// `incoming.<col>` — the proposed row of an upsert.
    pub incoming: bool,
}

impl Scope {
    pub(crate) const LITERALS: Self = Self {
        columns: false,
        incoming: false,
    };
    pub(crate) const ROW: Self = Self {
        columns: true,
        incoming: false,
    };
}
