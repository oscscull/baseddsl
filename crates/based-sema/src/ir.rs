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
    pub const DUP_SCOPE: &str = "E0105"; // duplicate `scope` decl name
    pub const DUP_ENUM: &str = "E0106"; // enum name collides with a model/shape/scope/enum
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
    pub const SHAPE_REF_UNKNOWN: &str = "E0132"; // `field -> Name` names no shape
    pub const SHAPE_REF_MODEL: &str = "E0133"; // referenced shape's model ≠ relation target
    pub const SHAPE_REF_CYCLE: &str = "E0134"; // a shape transitively nests itself by reference
                                               // queries / mutations
    pub const UNKNOWN_RETURN: &str = "E0140";
    pub const RETURN_MODEL_MISMATCH: &str = "E0141";
    pub const FULL_NEEDS_MODEL: &str = "E0142";
    pub const BINDING_EDGE: &str = "E0143"; // `-> edge` not a relation
    pub const GET_NOT_UNIQUE: &str = "E0144"; // get must key a unique field
    pub const RESTORE_NOT_SOFT: &str = "E0145";
    // create omits a required (non-optional, non-defaulted) column.
    pub const CREATE_MISSING: &str = "E0146";
    // operand typing
    pub const OP_TYPE: &str = "E0150"; // operator not applicable to the operand type
    pub const CMP_TYPE: &str = "E0151"; // incompatible operand types in a comparison
    pub const PARAM_TYPE: &str = "E0152"; // param annotation disagrees with its mapped column
    pub const ASSIGN_TYPE: &str = "E0153"; // create/update assigns a value of the wrong type to a column
    pub const ENUM_VARIANT: &str = "E0154"; // a where/create/update value is not a variant of the column's enum
    pub const ENUM_DEFAULT: &str = "E0155"; // a field's `default <variant>` is not a member of its enum (or the field isn't an enum)
    pub const ENUM_MIXED: &str = "E0156"; // an enum mixes an int-valued variant with a bare/string one (kind is ambiguous)
    pub const ENUM_DUP_VALUE: &str = "E0157"; // two variants of an enum share a wire value (string or int)
    pub const ENUM_ORDERED_OP: &str = "E0158"; // an ordered comparison (< > <= >=) on a string enum column

    // $ctx typing : the caller-supplied request context. Its type is not
    // declared — it is inferred per callable from use and checked for coherence.
    pub const CTX_BAD_PATH: &str = "E0160"; // $ctx used without exactly one field segment
    pub const CTX_CONFLICT: &str = "E0161"; // $ctx.<field> used at incompatible types across uses

    // tx back-references: `^.field` reads the immediately preceding
    // `create` in the same `tx`.
    pub const BACKREF_SCOPE: &str = "E0170"; // `^` outside a `tx`, or with no preceding `create`

    // Named scope: a `scope` decl referenced by
    // `@scope Name` on a model + `scoped Name` on every callable that touches it.
    pub const SCOPE_FORM: &str = "E0180"; // a `scope` decl's predicate isn't a conjunction of `col = $ctx.field`
    pub const SCOPE_ASSIGN: &str = "E0181"; // a `create` assigns a scope column (engine-managed)
    pub const SCOPE_MISSING_ACK: &str = "E0182"; // scoped callable writes neither `scoped …` nor `unscoped(…)`
    pub const SCOPE_UNKNOWN: &str = "E0183"; // `@scope Name` / `scoped Name` names no `scope` decl
    pub const SCOPE_MODEL_COLUMN: &str = "E0184"; // `@scope` model lacks the scope's column / wrong type
    pub const SCOPE_ACK_MISMATCH: &str = "E0185"; // `scoped …` set ⊉ any alternative of a touched scoped model
    pub const SCOPE_CREATE_UNSAT: &str = "E0186"; // a `create` can satisfy no alternative

    // `@was("old")` rename directive: declares a field's/model's
    // previous physical name so the diff emits a clean rename instead of drop+add.
    pub const WAS_NOOP: &str = "E0190"; // `@was` names the field's/model's own current name (a no-op)
    pub const WAS_LIVE: &str = "E0191"; // `@was("old")` but `old` is still a live column/table (can't be the rename source)
                                        // lints
    pub const NONDET_SORT: &str = "W0100";
    pub const UNKNOWN_DECORATOR: &str = "W0101";
    pub const RAW_SOFT_DELETE_GAP: &str = "W0102";
    // index lints
    pub const UNINDEXED: &str = "W0103"; // a query will scan: no usable index, no annotation
    pub const USELESS_INDEX: &str = "W0104"; // declared index no query uses (pure write-tax)
    pub const STALE_UNINDEXED: &str = "W0105"; // unindexed(...) on a query that is indexed
    pub const STALE_UNSCOPED: &str = "W0106"; // unscoped(...) on a callable whose model has no @scope
    pub const WAS_SPENT: &str = "W0107"; // `@was` rename already captured — remove it (offline, LSP)
    pub const MIGRATE_DRIFT: &str = "W0108"; // schema is ahead of migrations — run `based migrate gen` (offline, LSP)
}

/// The known model-level decorators. Anything else is a `W0101` (still a modifier,
/// just not one the engine understands).
pub const KNOWN_DECORATORS: &[&str] = &[
    "soft_delete",
    "sort",
    "scope",
    "created",
    "updated",
    "table",
    "was",
];

/// The closed set of value-position functions (the grammar leaves the set to sema).
pub const KNOWN_FUNCS: &[&str] = &["now"];

// ---------- resolved schema -----------------------------------------------

/// A checked, cross-linked schema: the IR seed for codegen.
#[derive(Debug, Default)]
pub struct CheckedSchema {
    pub models: Vec<RModel>,
    pub shapes: Vec<RShape>,
    /// Named scope decls, keyed by name in `scope_index`.
    pub scopes: Vec<RScope>,
    /// Enum decls, keyed by name in `enum_index`. A field typed by an enum name is a
    /// scalar column (`MemberKind::Scalar` carrying `enum_name`), never a relation.
    pub enums: Vec<REnum>,
    pub queries: Vec<RQuery>,
    pub mutations: Vec<RMutation>,
    pub filters: Vec<RFilter>,
    /// model name -> index into `models`.
    pub model_index: HashMap<String, usize>,
    /// scope name -> index into `scopes`.
    pub scope_index: HashMap<String, usize>,
    /// enum name -> index into `enums`.
    pub enum_index: HashMap<String, usize>,
}

impl CheckedSchema {
    pub fn model(&self, name: &str) -> Option<&RModel> {
        self.model_index.get(name).map(|&i| &self.models[i])
    }
    pub fn scope(&self, name: &str) -> Option<&RScope> {
        self.scope_index.get(name).map(|&i| &self.scopes[i])
    }
    pub fn enum_(&self, name: &str) -> Option<&REnum> {
        self.enum_index.get(name).map(|&i| &self.enums[i])
    }
}

/// A resolved `enum Name { … }` decl: its inferred kind and ordered variant list. The
/// variants are the closed member set every enum-typed value is checked against (by
/// name); each carries its wire value (a string or an int).
#[derive(Debug, Clone)]
pub struct REnum {
    pub name: String,
    pub span: Span,
    pub kind: EnumKind,
    pub variants: Vec<REnumVariant>,
}

/// An enum's kind, inferred from its variant values: `Str` when no variant carries an
/// int (bare or explicit-string variants — stored as text + CHECK), `Int` when every
/// variant carries an int (stored as an integer column + CHECK, ordered-comparable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumKind {
    Str,
    Int,
}

/// One resolved variant: the bare identifier name (the Rust variant / go-to-def target)
/// and its wire value.
#[derive(Debug, Clone)]
pub struct REnumVariant {
    pub name: String,
    pub span: Span,
    pub value: EnumValue,
}

/// A variant's wire representation: a string (a string enum) or an integer (an int enum).
#[derive(Debug, Clone, PartialEq)]
pub enum EnumValue {
    Str(String),
    Int(i64),
}

impl REnum {
    pub fn has_variant(&self, v: &str) -> bool {
        self.variants.iter().any(|x| x.name == v)
    }
    pub fn is_int(&self) -> bool {
        self.kind == EnumKind::Int
    }
    /// The wire value of a variant by name, or `None` if it names no variant.
    pub fn wire_of(&self, name: &str) -> Option<&EnumValue> {
        self.variants
            .iter()
            .find(|v| v.name == name)
            .map(|v| &v.value)
    }
    /// The variant names, comma-joinable for a diagnostic's "expected one of" list.
    pub fn variant_names(&self) -> Vec<&str> {
        self.variants.iter().map(|v| v.name.as_str()).collect()
    }
}

/// A resolved `scope` decl: its terms carry the column,
/// the `$ctx` field, and the type declared once here (the one source of truth for
/// both the governed models' column and the `$ctx.field`).
#[derive(Debug, Clone)]
pub struct RScope {
    pub name: String,
    pub span: Span,
    pub terms: Vec<RScopeTerm>,
}

#[derive(Debug, Clone)]
pub struct RScopeTerm {
    /// The scope column (the field name a governed model must carry).
    pub column: String,
    /// The `$ctx.<field>` the column binds to.
    pub ctx_field: String,
    /// The type declared in the decl (`col: Type`) — a primitive or a relation.
    pub ty: CtxField,
}

/// The scope injection a single callable chose for one touched scoped model .
/// A model may declare several `@scope` alternatives (DNF); the callable's `scoped …`
/// clause selects which axes confine *this* callable. `terms` is the flattened
/// `(column_field, ctx_field)` set of the chosen axes — exactly the equalities codegen
/// ANDs into the root `WHERE`, the joined `ON`, and the create auto-set for `model`.
/// Two callables naming different alternatives of the same model therefore inject
/// different predicates. For a single-alternative model this is that model's whole
/// scope, so the emitted SQL is unchanged from iteration 1 .
#[derive(Debug, Clone)]
pub struct ScopeInject {
    /// The touched scoped model this injection confines (by name).
    pub model: String,
    /// The `(column_field, ctx_field)` terms to inject, in scope-decl order, deduped.
    pub terms: Vec<(String, String)>,
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
    /// The standing scope filter injected into every read/write on this model
    /// Synthesized from the model's `@scope Name` reference(s) —
    /// the conjunction of the referenced `scope` decls' `col = $ctx.field` terms
    /// (the single alternative, iteration 1). `None` when the model is not scoped.
    /// Codegen lowers it exactly like any `where` , so scope injection is
    /// unchanged in effect from the old inline `@scope(pred)`.
    pub scope: Option<Predicate>,
    /// The model's `@scope` alternatives as scope-name sets (DNF): each
    /// `@scope Name[, Name]*` decorator is one alternative (an AND of names). Empty
    /// when the model is not scoped. Iteration 1 resolves exactly one alternative but
    /// stores a list so multi-scope  adds DNF without reshaping this.
    pub scope_alts: Vec<Vec<String>>,
    /// `@created` / `@updated` engine-managed timestamp fields .
    pub created: Option<String>,
    pub updated: Option<String>,
    pub indexes: Vec<RIndex>,
    /// Engine-inferred baseline indexes: FK columns of inverse
    /// edges the access layer actually traverses, minus anything a declared index
    /// already covers. Columns are field-level (like `indexes`); DDL prepends the
    /// soft-delete column (predicate-leading). Never `unique`.
    pub inferred_indexes: Vec<RIndex>,
    /// Field names that are individually unique (id, `(unique)`, single-col unique
    /// index). Drives `get`-must-be-keyed lint and codegen constraints.
    pub unique_cols: Vec<String>,
    /// `@was("old_table")` — the model's previous table name, driving a `rename table`
    /// step in the migration diff instead of drop+add. `None` for
    /// an un-renamed model. Transient: removed once the rename migration is captured.
    pub was: Option<String>,
}

impl RModel {
    pub fn member(&self, name: &str) -> Option<&RMember> {
        self.members.iter().find(|m| m.name == name)
    }
    /// Find a member by its *physical* column name (not the field name): a scalar's
    /// `column` or a forward relation's `fk_col`. Custom `on:` join conditions are
    /// written in terms of DB columns (legacy keys), so they resolve through this,
    /// not `member`.
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
    /// The `@scope` equality terms as `(lhs_field, ctx_field)` pairs : for
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

    /// The single `$ctx` field a request on this model **shards** on , or `None`
    /// when the model has no `@scope`. A scope is a conjunction of `col = $ctx.field`
    /// ; the shard key is the *owner* the scope filters by, i.e. the `$ctx` field
    /// of the **first** scope term (`@scope(org = $ctx.org)` → `Some("org")`). This is
    /// the one field the router hashes to pick a physical shard (single-shard-
    /// per-request), read from the same `@scope` that filters rows — one source of
    /// truth, so the shard a row lives in and the shard its owner's requests route to
    /// can never drift. A multi-term scope shards on its first `$ctx` field (the
    /// remaining terms narrow *within* that owner's shard); a model with no scope has
    /// no owning shard (single-shard deployments send it to shard 0).
    pub fn shard_key_ctx_field(&self) -> Option<String> {
        self.scope_terms().into_iter().next().map(|(_, ctx)| ctx)
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
    /// `@was("old_col")` — the field's previous physical column name, driving a
    /// `rename column` step in the migration diff. `None` for an
    /// un-renamed field. Transient: removed once the rename migration is captured.
    pub was: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MemberKind {
    /// A stored column. `column` is the physical name (`(column "…")` override or
    /// the field name verbatim).
    Scalar {
        ty: Primitive,
        optional: bool,
        many: bool,
        column: String,
        /// `(unique)` modifier — a single-column UNIQUE constraint at codegen.
        unique: bool,
        /// `(default …)` value, carried through for DDL column defaults.
        default: Option<DefaultVal>,
        /// The enum this column is typed by, when its declared type resolved to an
        /// `enum` decl (`status: Status`). `Some(name)` marks an enum-valued column —
        /// stored as text (`ty` is `Text`), constrained to the enum's variants, emitted
        /// as a real enum in the client. `None` for an ordinary primitive column.
        enum_name: Option<String>,
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

impl RMember {
    /// The member's physical column name: a scalar's `column`, a forward relation's
    /// `fk_col`, else the field name (an inverse owns no column). The rename target a
    /// `@was` maps its old name to.
    pub fn physical_col(&self) -> &str {
        match &self.kind {
            MemberKind::Scalar { column, .. } => column,
            MemberKind::Forward { fk_col, .. } => fk_col,
            MemberKind::Inverse { .. } => &self.name,
        }
    }
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
    /// Model the query reads from (inferred from the return shape).
    pub target: String,
    /// `get`/`list` — explicit in a block body, inferred from cardinality otherwise.
    pub verb: Verb,
    pub many: bool,
    /// The return shape, or `None` when the return type is a bare model.
    pub ret_shape: Option<String>,
    pub paginated: bool,
    /// The `$ctx.<field>` this query requires (its own `where` + the target model's
    /// `@scope` + expanded filters), each typed by inference . Deduped per
    /// callable; the client sends exactly these as request context.
    pub ctx_requires: Vec<CtxReq>,
    /// The `$ctx` field this query **shards** on : the target model's `@scope`
    /// owner field ([`RModel::shard_key_ctx_field`]), or `None` when the model has no
    /// `@scope` *or* the query is `unscoped` (a cross-scope read has no single
    /// owning shard, so it must route explicitly, never by a scope it disabled). The
    /// runtime pulls this field out of the request `$ctx` to route to one shard.
    pub shard_key: Option<String>,
    /// The per-touched-model scope injection this query chose: for
    /// each scoped model it reads (root + every joined reach), the terms of the
    /// alternative its `scoped …` clause satisfied. Empty when `unscoped` or nothing
    /// scoped is touched. Codegen injects exactly these, so a callable naming one
    /// alternative and another naming a different one filter by different predicates.
    pub scope_inject: Vec<ScopeInject>,
}

#[derive(Debug, Clone)]
pub struct RMutation {
    pub name: String,
    pub span: Span,
    pub ret_model: String,
    /// The return shape, or `None` when the return type is a bare model — the twin of
    /// [`RQuery::ret_shape`]. Codegen projects it when re-selecting the written row's
    /// declared shape after the write .
    pub ret_shape: Option<String>,
    /// The `$ctx.<field>` this mutation requires (its write `where`s + the write
    /// models' `@scope` + `create`/`update` assigns), each typed by inference
    /// Deduped per callable.
    pub ctx_requires: Vec<CtxReq>,
    /// The `$ctx` field this mutation **shards** on : the return model's `@scope`
    /// owner field ([`RModel::shard_key_ctx_field`]), or `None` when it has no `@scope`
    /// *or* the mutation is `unscoped` . A `tx` is a single-shard unit, so
    /// the whole mutation routes on this one field (the return model is the primary
    /// written model). The runtime pulls it out of the request `$ctx` to pick a shard.
    pub shard_key: Option<String>,
    /// The per-touched-model scope injection this mutation chose: the
    /// twin of [`RQuery::scope_inject`] for the write side — the chosen alternative's
    /// terms per written/joined scoped model, injected into every write `WHERE`, the
    /// joined `ON`, the create auto-set, and the declared-shape re-select. Empty when
    /// `unscoped`.
    pub scope_inject: Vec<ScopeInject>,
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

/// One `$ctx.<field>` requirement of a single callable : the field name and
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
/// caller supplies that model's key).
#[derive(Debug, Clone)]
pub enum CtxField {
    Scalar(Primitive),
    Relation(String),
}

/// Table name for a model : `snake_case(Name)`, no pluralization.
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
