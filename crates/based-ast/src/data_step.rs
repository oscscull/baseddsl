//! Typed migration data steps: the `create` / `update` / `delete` lines of an `up.mig`,
//! named by the snapshot's table and column names.

use super::*;

/// One data step, with its optional `chunk` modifier.
#[derive(Debug, Clone, PartialEq)]
pub struct DataStep {
    pub op: DataOp,
    pub chunk: Option<Chunk>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataOp {
    /// `create t { … }` (one row) or `create t[] from [ { … }, … ]` (literal rows), with an
    /// optional `on conflict (…) update { … }` upsert tail.
    Create {
        table: Ident,
        rows: Vec<DataRow>,
        bulk: bool,
        conflict: Option<DataConflict>,
    },
    /// `update t where (…) { … }`, or `update all t { … }` (`where_` = `None`).
    Update {
        table: Ident,
        where_: Option<Predicate>,
        assigns: Vec<DataAssign>,
    },
    /// `delete t where (…)`, or `delete all t` (`where_` = `None`).
    Delete {
        table: Ident,
        where_: Option<Predicate>,
    },
}

impl DataOp {
    pub fn table(&self) -> &Ident {
        match self {
            Self::Create { table, .. }
            | Self::Update { table, .. }
            | Self::Delete { table, .. } => table,
        }
    }
}

/// `col = expr` — the RHS is the generated-column expression language (literals, same-row
/// columns, JSON paths, `+ - * /`, `||`, `case`).
#[derive(Debug, Clone, PartialEq)]
pub struct DataAssign {
    pub col: Ident,
    pub value: ShapeExpr,
}

/// One literal row of a `create`.
#[derive(Debug, Clone, PartialEq)]
pub struct DataRow {
    pub assigns: Vec<DataAssign>,
    pub span: Span,
}

/// `on conflict (target) update { … }` on a data-step `create`.
#[derive(Debug, Clone, PartialEq)]
pub struct DataConflict {
    pub target: Vec<Ident>,
    pub update: Vec<DataAssign>,
    /// `...incoming`: every inserted column (minus the target) from the proposed row.
    pub spread: Option<ConflictSpread>,
    pub span: Span,
}

/// `chunk N [by col] [pause <duration>]` — run the step in key-ordered batches of `size`
/// rows, one transaction per batch.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub size: u64,
    pub by: Option<Ident>,
    pub pause_ms: Option<u64>,
    pub span: Span,
}
