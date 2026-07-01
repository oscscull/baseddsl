//! Resolved schema IR + diagnostic codes.
//!
//! `check()` turns the flat `[Decl]` into this cross-linked form: models with
//! their implicit columns, resolved relations, and engine-managed roles, plus
//! resolved summaries of every shape / query / mutation / filter. It is the seed
//! codegen reads (alongside the AST) — the resolution facts that are *not* in the
//! AST (inferred verb, relation targets, table names, soft-delete mode) live here.

use based_ast::{Predicate, Primitive, SortTerm, Span, Verb};
use based_diagnostics::Diagnostic;
use std::collections::HashMap;

// ---------- diagnostic codes ----------------------------------------------
// E01xx = sema errors, W01xx = sema lints. Parser owns E0001/E0002, manifest
// E001x. Codes are stable so lints can be referenced in the spec and ratcheted.
pub mod code {
    // resolution / uniqueness
    pub const DUP_MODEL: &str = "E0100";
    pub const DUP_SHAPE: &str = "E0101";
    pub const DUP_CALLABLE: &str = "E0102"; // query/mutation share the wire namespace
    pub const DUP_FILTER: &str = "E0103";
    pub const DUP_FIELD: &str = "E0104";
    pub const UNKNOWN_MODEL: &str = "E0110";
    pub const UNKNOWN_FIELD: &str = "E0111";
    pub const TRAVERSE_SCALAR: &str = "E0112"; // dotted past a scalar column
    pub const UNKNOWN_PARAM: &str = "E0113";
    pub const UNKNOWN_FILTER: &str = "E0114";
    pub const FILTER_ARITY: &str = "E0115";
    pub const UNKNOWN_FUNC: &str = "E0116";
    // models
    pub const SOFT_DELETE_TYPE: &str = "E0120"; // field not in the covered subset
    pub const DECO_TARGET: &str = "E0121"; // @created/@updated/@tenant/@soft_delete target
    pub const INDEX_COLUMN: &str = "E0122";
    pub const INVERSE_REF: &str = "E0123"; // (Model.field) does not name a forward edge
    pub const INVERSE_INFER: &str = "E0124"; // to-many with no inferable / ambiguous inverse
    // shapes
    pub const SHAPE_BARE_RELATION: &str = "E0130"; // bare relation must nest or `=`
    pub const SHAPE_NEST_SCALAR: &str = "E0131"; // nested a non-relation
    // queries / mutations
    pub const UNKNOWN_RETURN: &str = "E0140";
    pub const RETURN_MODEL_MISMATCH: &str = "E0141";
    pub const FULL_NEEDS_MODEL: &str = "E0142";
    pub const BINDING_EDGE: &str = "E0143"; // `-> edge` not a relation
    pub const GET_NOT_UNIQUE: &str = "E0144"; // get must key a unique field
    pub const RESTORE_NOT_SOFT: &str = "E0145";
    // lints
    pub const NONDET_SORT: &str = "W0100";
    pub const UNKNOWN_DECORATOR: &str = "W0101";
    pub const RAW_SOFT_DELETE_GAP: &str = "W0102";
}

/// The known model-level decorators. Anything else is a `W0101` (still a modifier,
/// just not one the engine understands — models.md).
pub const KNOWN_DECORATORS: &[&str] = &[
    "soft_delete",
    "sort",
    "tenant",
    "scope",
    "created",
    "updated",
    "table",
];

/// The closed set of value-position functions (grammar defers the set to sema).
pub const KNOWN_FUNCS: &[&str] = &["now"];

// ---------- resolved schema -----------------------------------------------

/// A checked, cross-linked schema: the IR seed for codegen.
#[derive(Debug, Default)]
pub struct CheckedSchema {
    pub models: Vec<RModel>,
    pub shapes: Vec<RShape>,
    pub queries: Vec<RQuery>,
    pub mutations: Vec<RMutation>,
    pub filters: Vec<RFilter>,
    /// model name -> index into `models`.
    pub model_index: HashMap<String, usize>,
}

impl CheckedSchema {
    pub fn model(&self, name: &str) -> Option<&RModel> {
        self.model_index.get(name).map(|&i| &self.models[i])
    }
}

#[derive(Debug, Clone)]
pub struct RModel {
    pub name: String,
    pub span: Span,
    /// Generated table name (`snake_case`) or the `@table("…")` override.
    pub table: String,
    pub members: Vec<RMember>,
    pub soft_delete: Option<SoftDelete>,
    /// Model default sort (`@sort`); empty when none is declared.
    pub sort: Vec<SortTerm>,
    /// `@scope(pred)` — a standing auth filter injected into every query (auth.md).
    pub scope: Option<Predicate>,
    /// `@tenant(field)` — the tenant relation field, if declared.
    pub tenant: Option<String>,
    /// `@created` / `@updated` engine-managed timestamp fields (D2).
    pub created: Option<String>,
    pub updated: Option<String>,
    pub indexes: Vec<RIndex>,
    /// Field names that are individually unique (id, `(unique)`, single-col unique
    /// index). Drives `get`-must-be-keyed lint and codegen constraints.
    pub unique_cols: Vec<String>,
}

impl RModel {
    pub fn member(&self, name: &str) -> Option<&RMember> {
        self.members.iter().find(|m| m.name == name)
    }
    pub fn is_unique(&self, field: &str) -> bool {
        self.unique_cols.iter().any(|c| c == field)
    }
}

/// One resolved field: a scalar column or a relation edge.
#[derive(Debug, Clone)]
pub struct RMember {
    pub name: String,
    pub span: Span,
    pub kind: MemberKind,
}

#[derive(Debug, Clone)]
pub enum MemberKind {
    /// A stored column. `column` is the physical name (`(column "…")` override or
    /// the field name verbatim, D3).
    Scalar {
        ty: Primitive,
        optional: bool,
        many: bool,
        column: String,
    },
    /// To-one relation: FK lives on this table (`<field>_id`, or a custom join).
    Forward {
        target: String,
        optional: bool,
        fk_col: String,
        custom_join: bool,
    },
    /// Back edge (to-many, or a one-to-one inverse): FK lives on `target`, paired
    /// with its forward field `via`.
    Inverse { target: String, via: String },
}

impl MemberKind {
    pub fn is_relation(&self) -> bool {
        !matches!(self, MemberKind::Scalar { .. })
    }
    pub fn target(&self) -> Option<&str> {
        match self {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => Some(target),
            MemberKind::Scalar { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftMode {
    /// nullable `timestamp`/`date`: live `IS NULL`.
    Timestamp,
    /// `bool`: live `= false`.
    Bool,
}

#[derive(Debug, Clone)]
pub struct SoftDelete {
    pub field: String,
    pub mode: SoftMode,
}

#[derive(Debug, Clone)]
pub struct RIndex {
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone)]
pub struct RShape {
    pub name: String,
    pub from: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RQuery {
    pub name: String,
    pub span: Span,
    /// Model the query reads from (inferred from the return shape, queries.md).
    pub target: String,
    /// `get`/`list` — explicit in a block body, inferred from cardinality otherwise.
    pub verb: Verb,
    pub many: bool,
    /// The return shape, or `None` when the return type is a bare model.
    pub ret_shape: Option<String>,
    pub paginated: bool,
}

#[derive(Debug, Clone)]
pub struct RMutation {
    pub name: String,
    pub span: Span,
    pub ret_model: String,
}

#[derive(Debug, Clone)]
pub struct RFilter {
    pub name: String,
    pub span: Span,
    pub arity: usize,
}

// ---------- diagnostics sink ----------------------------------------------

/// Thin accumulator so passes can push errors/warnings without ceremony.
#[derive(Default)]
pub struct Sink {
    pub diags: Vec<Diagnostic>,
}

impl Sink {
    pub fn error(&mut self, code: &'static str, span: Span, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(code, msg).at(span));
    }
    pub fn warn(&mut self, code: &'static str, span: Span, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::warning(code, msg).at(span));
    }
    pub fn error_note(
        &mut self,
        code: &'static str,
        span: Span,
        msg: impl Into<String>,
        note: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, msg).at(span).note(note));
    }
}

/// Table name for a model (D3): `snake_case(Name)`, no pluralization.
pub fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.char_indices() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}
