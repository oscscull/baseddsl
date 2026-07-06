//! Resolved schema IR + diagnostic codes.
//!
//! `check()` turns the flat `[Decl]` into this cross-linked form: models with
//! their implicit columns, resolved relations, and engine-managed roles, plus
//! resolved summaries of every shape / query / mutation / filter. It is the seed
//! codegen reads (alongside the AST) — the resolution facts that are *not* in the
//! AST (inferred verb, relation targets, table names, soft-delete mode) live here.

use based_ast::{DefaultVal, Predicate, Primitive, SortTerm, Span, Verb};
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
    pub const DECO_TARGET: &str = "E0121"; // @created/@updated/@soft_delete target
    pub const INDEX_COLUMN: &str = "E0122";
    pub const INVERSE_REF: &str = "E0123"; // (Model.field) does not name a forward edge
    pub const INVERSE_INFER: &str = "E0124"; // to-many with no inferable / ambiguous inverse
    pub const JOIN_TABLE: &str = "E0125"; // custom `on:` join names a table not in scope
    pub const JOIN_FORM: &str = "E0126"; // custom `on:` join malformed (not `<table>.<col>`, or not a to-one relation)
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
    // create omits a required (non-optional, non-defaulted) column.
    pub const CREATE_MISSING: &str = "E0146";
    // operand typing (PLAN.md sema #1)
    pub const OP_TYPE: &str = "E0150"; // operator not applicable to the operand type
    pub const CMP_TYPE: &str = "E0151"; // incompatible operand types in a comparison
    pub const PARAM_TYPE: &str = "E0152"; // param annotation disagrees with its mapped column (D1)
    pub const ASSIGN_TYPE: &str = "E0153"; // create/update assigns a value of the wrong type to a column

    // $ctx typing (D4/D5): the caller-supplied request context. Its type is not
    // declared — it is inferred per callable from use and checked for coherence.
    pub const CTX_BAD_PATH: &str = "E0160"; // $ctx used without exactly one field segment
    pub const CTX_CONFLICT: &str = "E0161"; // $ctx.<field> used at incompatible types across uses

    // tx back-references (mutations.md): `^.field` reads the immediately preceding
    // `create` in the same `tx`.
    pub const BACKREF_SCOPE: &str = "E0170"; // `^` outside a `tx`, or with no preceding `create`

    // `@scope` (auth.md Handle 2 / D32): a uniform single-owner row filter.
    pub const SCOPE_FORM: &str = "E0180"; // @scope must be a conjunction of `col = $ctx.field`
    pub const SCOPE_ASSIGN: &str = "E0181"; // a `create` assigns a scope column (engine-managed)
                                            // lints
    pub const NONDET_SORT: &str = "W0100";
    pub const UNKNOWN_DECORATOR: &str = "W0101";
    pub const RAW_SOFT_DELETE_GAP: &str = "W0102";
    // index lints (indexing.md, D15)
    pub const UNINDEXED: &str = "W0103"; // a query will scan: no usable index, no annotation
    pub const USELESS_INDEX: &str = "W0104"; // declared index no query uses (pure write-tax)
    pub const STALE_UNINDEXED: &str = "W0105"; // unindexed(...) on a query that is indexed
    pub const STALE_UNSCOPED: &str = "W0106"; // unscoped(...) on a callable whose model has no @scope
}

/// The known model-level decorators. Anything else is a `W0101` (still a modifier,
/// just not one the engine understands — models.md).
pub const KNOWN_DECORATORS: &[&str] = &[
    "soft_delete",
    "sort",
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
    /// `@created` / `@updated` engine-managed timestamp fields (D2).
    pub created: Option<String>,
    pub updated: Option<String>,
    pub indexes: Vec<RIndex>,
    /// Engine-inferred baseline indexes (indexing.md, D15): FK columns of inverse
    /// edges the access layer actually traverses, minus anything a declared index
    /// already covers. Columns are field-level (like `indexes`); DDL prepends the
    /// soft-delete column (predicate-leading). Never `unique`.
    pub inferred_indexes: Vec<RIndex>,
    /// Field names that are individually unique (id, `(unique)`, single-col unique
    /// index). Drives `get`-must-be-keyed lint and codegen constraints.
    pub unique_cols: Vec<String>,
}

impl RModel {
    pub fn member(&self, name: &str) -> Option<&RMember> {
        self.members.iter().find(|m| m.name == name)
    }
    /// Find a member by its *physical* column name (not the field name): a scalar's
    /// `column` or a forward relation's `fk_col`. Custom `on:` join conditions are
    /// written in terms of DB columns (legacy keys), so they resolve through this,
    /// not `member` (relations.md).
    pub fn column(&self, col: &str) -> Option<&RMember> {
        self.members.iter().find(|m| match &m.kind {
            MemberKind::Scalar { column, .. } => column == col,
            MemberKind::Forward { fk_col, .. } => fk_col == col,
            MemberKind::Inverse { .. } => false,
        })
    }
    pub fn is_unique(&self, field: &str) -> bool {
        self.unique_cols.iter().any(|c| c == field)
    }
    /// The `@scope` equality terms as `(lhs_field, ctx_field)` pairs (D32): for
    /// `@scope(org = $ctx.org)`, `[("org", "org")]`. Sema restricts `@scope` to a
    /// conjunction of `col = $ctx.field` (`E0180`), so this is exactly the set of
    /// columns the engine injects into every read/write and **auto-sets on create**
    /// from `:ctx_<ctx_field>`. Empty when the model has no `@scope`. A malformed
    /// scope (already `E0180`) contributes only its well-formed terms.
    pub fn scope_terms(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(p) = &self.scope {
            collect_scope_terms(p, &mut out);
        }
        out
    }
}

/// Flatten a well-formed `@scope` predicate (an `and`-tree of `col = $ctx.field`) into
/// its `(lhs_field, ctx_field)` terms. Non-conforming nodes are skipped (they are the
/// `E0180` the caller already reported); this never errors.
fn collect_scope_terms(p: &Predicate, out: &mut Vec<(String, String)>) {
    match p {
        Predicate::And(a, b) => {
            collect_scope_terms(a, out);
            collect_scope_terms(b, out);
        }
        Predicate::Cmp {
            path,
            op: based_ast::Op::Eq,
            value: based_ast::Value::Param(pr),
        } if path.segments.len() == 1 && pr.name.node == "ctx" && pr.path.len() == 1 => {
            out.push((path.segments[0].node.clone(), pr.path[0].node.clone()));
        }
        _ => {}
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
        /// `(unique)` modifier — a single-column UNIQUE constraint at codegen.
        unique: bool,
        /// `(default …)` value, carried through for DDL column defaults.
        default: Option<DefaultVal>,
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
    /// The `$ctx.<field>` this query requires (its own `where` + the target model's
    /// `@scope` + expanded filters), each typed by inference (D4/D5). Deduped per
    /// callable; the client sends exactly these as request context.
    pub ctx_requires: Vec<CtxReq>,
}

#[derive(Debug, Clone)]
pub struct RMutation {
    pub name: String,
    pub span: Span,
    pub ret_model: String,
    /// The return shape, or `None` when the return type is a bare model — the twin of
    /// [`RQuery::ret_shape`]. Codegen projects it when re-selecting the written row's
    /// declared shape after the write (D12).
    pub ret_shape: Option<String>,
    /// The `$ctx.<field>` this mutation requires (its write `where`s + the write
    /// models' `@scope` + `create`/`update` assigns), each typed by inference
    /// (D4/D5). Deduped per callable.
    pub ctx_requires: Vec<CtxReq>,
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
        self.diags.push(Diagnostic::error(code, msg).at(span));
    }
    pub fn warn(&mut self, code: &'static str, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::warning(code, msg).at(span));
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
    pub fn warn_note(
        &mut self,
        code: &'static str,
        span: Span,
        msg: impl Into<String>,
        note: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::warning(code, msg).at(span).note(note));
    }
}

/// One `$ctx.<field>` requirement of a single callable (D4/D5): the field name and
/// the type it was used at, inferred from the column the use compared against.
/// `$ctx` is per-request; there is no global context type — each query/mutation
/// requires exactly the fields *it* (plus its `@scope`/filters) reads. Cross-
/// callable coherence (a field must mean one type everywhere the caller's context
/// bag is shared) is checked separately (`CTX_CONFLICT`).
#[derive(Debug, Clone)]
pub struct CtxReq {
    pub field: String,
    pub ty: CtxField,
    pub span: Span,
}

/// A `$ctx` field's inferred type: a primitive, or a relation to a model (the
/// caller supplies that model's key, D1).
#[derive(Debug, Clone)]
pub enum CtxField {
    Scalar(Primitive),
    Relation(String),
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
