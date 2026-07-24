//! Path resolution + the shared predicate/value checker.
//!
//! One expression language is used in `where`, `@scope`, named filters, and
//! relation joins, so this module is the single place paths, params,
//! filter calls, and functions are validated. Path resolution walks declared
//! fields: forward *and* backward edges are just fields, so forward traversal
//! needs no inverse and backward traversal works exactly because the inverse was
//! declared.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;

/// Read-only resolution context shared by every checker pass.
pub struct Cx<'a> {
    pub models: &'a [RModel],
    /// model name -> index into `models`.
    pub index: &'a HashMap<String, usize>,
    /// named filter -> its declaration (arity + body). The body is re-resolved
    /// against each call-site model, since a filter has no model of its own.
    pub filters: &'a HashMap<String, &'a NamedFilter>,
    /// shape name -> the model it projects (`from`). Used to resolve return types.
    pub shapes: &'a HashMap<String, String>,
    /// shape name -> its projection body. Lets `$ctx` collection  walk a return
    /// shape's relation reaches to find joined *scoped* models, whose `@scope` codegen
    /// injects into the join `ON` — so the callable must require their `$ctx` fields.
    pub shape_bodies: &'a HashMap<String, &'a [ShapeField]>,
    /// Resolved `scope` decls — for validating a callable's
    /// `scoped Name` acknowledgement.
    pub scopes: &'a [RScope],
    /// scope name -> index into `scopes`.
    pub scope_index: &'a HashMap<String, usize>,
    /// Resolved `enum` decls — for validating variant membership in a value position.
    pub enums: &'a [REnum],
    /// enum name -> index into `enums`.
    pub enum_index: &'a HashMap<String, usize>,
}

impl<'a> Cx<'a> {
    pub fn model(&self, i: usize) -> &RModel {
        &self.models[i]
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }
    pub fn scope(&self, name: &str) -> Option<&RScope> {
        self.scope_index.get(name).map(|&i| &self.scopes[i])
    }
    pub fn enum_(&self, name: &str) -> Option<&REnum> {
        self.enum_index.get(name).map(|&i| &self.enums[i])
    }

    /// The enum a dotted path terminates on, when the terminal column is enum-typed
    /// (`where status = …`, `where placed_by.role = …`). `None` when the path is
    /// unresolvable or lands on a non-enum column/relation. Read-only — materializes no
    /// join (the caller's `resolve_path` already reported any name error).
    pub fn terminal_enum(&self, path: &Path, start: usize) -> Option<&REnum> {
        let mut cur = start;
        let n = path.segments.len();
        for (i, seg) in path.segments.iter().enumerate() {
            let mem = self.model(cur).member(&seg.node)?;
            let last = i + 1 == n;
            match &mem.kind {
                MemberKind::Scalar {
                    enum_name: Some(name),
                    ..
                } if last => return self.enum_(name),
                MemberKind::Scalar { .. } => return None,
                MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                    if last {
                        return None;
                    }
                    cur = self.find(target)?;
                }
            }
        }
        None
    }
}

/// Check a value written against an enum-typed column (`where status = paid`,
/// `create { status: paid }`). A bare single-segment identifier is a variant — checked
/// for membership (`E0154`). A `$param` is still name-checked. Returns `true` when it
/// fully handled the value (so the caller skips the ordinary column-path resolution that
/// would misread the variant as a field); `false` to fall through (a string literal, a
/// null, etc., which the ordinary text-family check then covers).
pub fn check_enum_operand(value: &Value, en: &REnum, params: &[String], sink: &mut Sink) -> bool {
    match value {
        Value::Path(p) if p.segments.len() == 1 => {
            let seg = &p.segments[0];
            if !en.has_variant(&seg.node) {
                sink.error(
                    code::ENUM_VARIANT,
                    seg.span,
                    format!(
                        "`{}` is not a variant of enum `{}` (expected one of: {})",
                        seg.node,
                        en.name,
                        en.variant_names().join(", ")
                    ),
                );
            }
            true
        }
        Value::Param(pr) => {
            check_param_ref(pr, params, sink);
            true
        }
        _ => false,
    }
}

/// What a dotted path lands on: the resolved type/target, consumed by the operand
/// type-checker below.
pub enum Terminal {
    Scalar(Primitive),
    /// A relation edge; `.0` is the target model name. Comparable to its key (Id).
    Relation(String),
    /// An opaque `raw(…)` column; `.0` is the declared type spelling. Projectable, but
    /// never a filter/sort/group/aggregate operand (`E0271`).
    Opaque(String),
}

/// A coarse operand-compatibility bucket. Deliberately loose — the goal is to
/// catch nonsense (`~` on an int, `age = "x"`, a relation compared with `<`), not
/// to police every SQL coercion. Timestamp/Date/Uuid/Id ride with text because
/// they are string-writable *and* orderable, which is exactly the set of ops we
/// allow on them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Textual,
    Numeric,
    Bool,
    Json, // holds anything → never mismatches
    Key,  // a relation edge, compared to its key (uuid string or int)
}

fn prim_family(ty: Primitive) -> Family {
    match ty {
        Primitive::Text
        | Primitive::Uuid
        | Primitive::Id
        | Primitive::Timestamp
        | Primitive::Date => Family::Textual,
        Primitive::Int | Primitive::Float | Primitive::Decimal { .. } => Family::Numeric,
        Primitive::Bool => Family::Bool,
        Primitive::Json => Family::Json,
    }
}

fn terminal_family(t: &Terminal) -> Family {
    match t {
        Terminal::Scalar(p) => prim_family(*p),
        Terminal::Relation(_) => Family::Key,
        // Never reached in a comparison — an opaque operand is rejected first (E0271).
        Terminal::Opaque(_) => Family::Json,
    }
}

/// Human-facing name for a resolved operand, for diagnostics.
fn terminal_name(t: &Terminal) -> String {
    match t {
        Terminal::Scalar(p) => format!("`{}`", prim_name(*p)),
        Terminal::Relation(m) => format!("relation `{m}`"),
        Terminal::Opaque(t) => format!("opaque column `{t}`"),
    }
}

fn prim_name(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Json => "json",
        Primitive::Uuid => "uuid",
        Primitive::Id => "id",
        Primitive::Float => "float",
        Primitive::Decimal { .. } => "decimal",
    }
}

fn lit_family(l: &Literal) -> Option<Family> {
    match l {
        Literal::Str(_) => Some(Family::Textual),
        Literal::Int(_) | Literal::Decimal(_) => Some(Family::Numeric),
        Literal::Bool(_) => Some(Family::Bool),
        Literal::Null => None, // null is compatible with any operand
    }
}

/// Are two operand families comparable with `=`/`!=` (and, for orderable ones, the
/// relational operators)? Json matches anything; a relation key accepts either a
/// uuid string or an integer key.
fn compatible(a: Family, b: Family) -> bool {
    use Family::{Json, Key, Numeric, Textual};
    if a == Json || b == Json {
        return true;
    }
    match (a, b) {
        (Key, Textual | Numeric) | (Textual | Numeric, Key) => true,
        _ => a == b,
    }
}

/// Why a column is ineligible for an aggregate (shapes.md), or `None` when it is fine.
/// `sum`/`avg` need the numeric family; `min`/`max` need a *comparable* column (numeric,
/// `timestamp`, `date`, `text`); `count` is arg-less so never reaches here. An enum or a
/// relation is never a numeric/comparable aggregate operand.
pub fn agg_operand_reason(func: &str, term: &Terminal, is_enum: bool) -> Option<String> {
    let numeric = matches!(
        term,
        Terminal::Scalar(Primitive::Int | Primitive::Float | Primitive::Decimal { .. })
    ) && !is_enum;
    let comparable = numeric
        || matches!(
            term,
            Terminal::Scalar(Primitive::Timestamp | Primitive::Date | Primitive::Text)
        ) && !is_enum;
    match func {
        "sum" | "avg" if !numeric => Some(format!(
            "`{func}` needs a numeric column (int/float/decimal), not {}",
            terminal_name(term)
        )),
        "min" | "max" if !comparable => Some(format!(
            "`{func}` needs a comparable column (numeric/timestamp/date/text), not {}",
            terminal_name(term)
        )),
        _ => None,
    }
}

/// Resolve a path for its type only, without reporting — name errors are already
/// surfaced by the caller's `resolve_path`, so the type pass stays silent to avoid
/// double-reporting.
fn resolve_quiet(path: &Path, start: usize, cx: &Cx) -> Option<Terminal> {
    let mut throwaway = Sink::default();
    resolve_path(path, start, cx, &mut throwaway)
}

/// Type-check one `Cmp`: the operator must apply to the left operand's type, and
/// (for equality/ordering against a literal or another column) the two operands
/// must share a family. Silent when either side failed to resolve — that name
/// error was already reported.
fn check_cmp_types(path: &Path, op: Op, value: &Value, mi: usize, cx: &Cx, sink: &mut Sink) {
    let Some(lhs) = resolve_quiet(path, mi, cx) else {
        return;
    };
    let Some(seg) = path.segments.last() else {
        return;
    };
    let span = seg.span;
    if reject_opaque(&lhs, path, "filter", sink) {
        return;
    }

    // 1. Operator applicability on the left operand.
    match op {
        Op::Like => {
            if !matches!(lhs, Terminal::Scalar(Primitive::Text)) {
                sink.error(
                    code::OP_TYPE,
                    span,
                    format!(
                        "`~` (like) needs a text column, but {} is not text",
                        terminal_name(&lhs)
                    ),
                );
                return;
            }
        }
        Op::Gt | Op::Lt | Op::Ge | Op::Le => {
            if matches!(
                terminal_family(&lhs),
                Family::Bool | Family::Json | Family::Key
            ) {
                sink.error(
                    code::OP_TYPE,
                    span,
                    format!(
                        "`{}` needs an orderable column, but {} is not",
                        op_sym(op),
                        terminal_name(&lhs)
                    ),
                );
                return;
            }
        }
        // `in`/`has` are collection/json containment: the right operand's element
        // type differs from the column, so family compatibility does not apply.
        Op::In | Op::Has => return,
        Op::Eq | Op::Ne => {}
    }

    // 2. Operand family compatibility against a literal or another column.
    let lf = terminal_family(&lhs);
    let rf = match value {
        Value::Lit(l) => match lit_family(l) {
            Some(f) => f,
            None => return, // null: no constraint
        },
        Value::Path(p) => match resolve_quiet(p, mi, cx) {
            Some(t) => terminal_family(&t),
            None => return, // unresolved RHS: name error already reported
        },
        // A param's type is checked at its declaration (check_param); a `$ctx.field`
        // is typed by inference from *this* comparison (ctx.rs), so it never clashes
        // here; a function's return type is not modelled yet.
        Value::Param(_) | Value::Func(_) => return,
    };
    if !compatible(lf, rf) {
        sink.error(
            code::CMP_TYPE,
            span,
            format!(
                "cannot compare {} to a {} value",
                terminal_name(&lhs),
                family_name(rf)
            ),
        );
    }
}

/// Type-check one `in` value-list element: unlike the single-bind `in $param`
/// form (whose RHS the engine can't see into), a listed element is compared to
/// the column with `=` semantics, so it must share the column's family — the
/// per-element twin of `check_cmp_types` step 2.
fn check_in_element_type(path: &Path, value: &Value, mi: usize, cx: &Cx, sink: &mut Sink) {
    let Some(lhs) = resolve_quiet(path, mi, cx) else {
        return;
    };
    let Some(seg) = path.segments.last() else {
        return;
    };
    if reject_opaque(&lhs, path, "filter", sink) {
        return;
    }
    let rf = match value {
        Value::Lit(l) => match lit_family(l) {
            Some(f) => f,
            None => return, // null: no constraint
        },
        Value::Path(p) => match resolve_quiet(p, mi, cx) {
            Some(t) => terminal_family(&t),
            None => return, // unresolved element: name error already reported
        },
        // Same skips as the single-value comparison: params are typed at their
        // declaration, `$ctx` by inference, functions are unmodelled.
        Value::Param(_) | Value::Func(_) => return,
    };
    if !compatible(terminal_family(&lhs), rf) {
        sink.error(
            code::CMP_TYPE,
            seg.span,
            format!(
                "cannot compare {} to a {} value",
                terminal_name(&lhs),
                family_name(rf)
            ),
        );
    }
}

/// The family a column accepts on assignment: a scalar's primitive family, or a
/// forward relation FK's key. An inverse edge owns no column, so it can't be
/// assigned — `None` skips the check (the misuse, if any, is out of scope here).
fn member_family(kind: &MemberKind) -> Option<Family> {
    match kind {
        // An opaque column is never assignable (E0273), so it contributes no family.
        MemberKind::Scalar {
            raw_type: Some(_), ..
        } => None,
        MemberKind::Scalar { ty, .. } => Some(prim_family(*ty)),
        MemberKind::Forward { .. } => Some(Family::Key),
        MemberKind::Inverse { .. } => None,
    }
}

/// Type-check one `create`/`update` assignment: the value's family must agree with
/// the target column's — the write-side twin of the `=` compatibility rule
/// (`check_cmp_types`). A literal or another column is family-checked. Params (typed at
/// their declaration / `$ctx` inferred) and functions (return type unmodelled) are
/// skipped, exactly as on the read side. Silent when a side fails to resolve — that
/// name error is already reported by the caller. A `$step.field` tx binding reference is
/// checked separately (`check::check_binding_ref` → `check_field_assign_type`).
pub fn check_assign_type(
    target: &MemberKind,
    col: &Ident,
    value: &Value,
    mi: usize,
    cx: &Cx,
    sink: &mut Sink,
) {
    let Some(lf) = member_family(target) else {
        return;
    };
    let rf = match value {
        Value::Lit(l) => match lit_family(l) {
            Some(f) => f,
            None => return, // null: no constraint
        },
        Value::Path(p) => match resolve_quiet(p, mi, cx) {
            Some(t) => terminal_family(&t),
            None => return, // unresolved column: name error already reported
        },
        Value::Param(_) | Value::Func(_) => return,
    };
    report_assign_family(target, col, rf, lf, sink);
}

/// Family-check a `$step.field` tx binding assignment (D107): the bound field's family
/// must agree with the target column's, the same rule `check_assign_type` applies to a
/// column/literal RHS. Silent when either side has no modellable family.
pub fn check_field_assign_type(
    target: &MemberKind,
    col: &Ident,
    source: &MemberKind,
    sink: &mut Sink,
) {
    let (Some(lf), Some(rf)) = (member_family(target), member_family(source)) else {
        return;
    };
    report_assign_family(target, col, rf, lf, sink);
}

/// Emit `E0153` when an assigned value's family (`rf`) is incompatible with the target
/// column's (`lf`). Shared by the column/literal and the tx-binding assign checks.
fn report_assign_family(target: &MemberKind, col: &Ident, rf: Family, lf: Family, sink: &mut Sink) {
    if !compatible(lf, rf) {
        let target_desc = match target {
            MemberKind::Scalar { ty, .. } => format!("`{}`", prim_name(*ty)),
            MemberKind::Forward { target, .. } => format!("relation `{target}`"),
            MemberKind::Inverse { .. } => return,
        };
        sink.error(
            code::ASSIGN_TYPE,
            col.span,
            format!(
                "cannot assign a {} value to `{}` (a {} column)",
                family_name(rf),
                col.node,
                target_desc
            ),
        );
    }
}

/// Type-check an arithmetic assignment RHS (`total = total + $n`, mutations.md).
/// Numeric-only and update-only: a `create` has no existing row to self-reference
/// (`E0230`); every column operand must be numeric (`E0231`); and, via the ordinary
/// assign-type rule, the target column must be numeric too (`E0153`). Params and
/// functions are typed at their declaration / unmodelled, so they are skipped here —
/// exactly as on every other write-side family check.
#[allow(clippy::too_many_arguments)]
pub fn check_assign_arith(
    rhs: &AssignRhs,
    target: &MemberKind,
    col: &Ident,
    mi: usize,
    in_update: bool,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    if !in_update {
        if let AssignRhs::Arith { span, .. } = rhs {
            sink.error(
                code::ARITH_CREATE,
                *span,
                "an arithmetic expression needs an existing row — valid only in `update`, \
                 not `create`"
                    .to_string(),
            );
        }
        return;
    }
    if member_family(target) != Some(Family::Numeric) {
        let target_desc = match target {
            MemberKind::Scalar { ty, .. } => format!("a `{}` column", prim_name(*ty)),
            MemberKind::Forward { target, .. } => format!("relation `{target}`"),
            MemberKind::Inverse { .. } => return,
        };
        sink.error(
            code::ASSIGN_TYPE,
            col.span,
            format!(
                "cannot assign an arithmetic (numeric) expression to `{}` ({target_desc})",
                col.node
            ),
        );
    }
    check_arith_operands(rhs, col, mi, cx, params, sink);
}

/// Walk an arithmetic RHS: resolve each leaf value (names, params, back-refs) and
/// require every column / literal operand to be numeric (`E0231`).
fn check_arith_operands(
    rhs: &AssignRhs,
    col: &Ident,
    mi: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    match rhs {
        AssignRhs::Arith { lhs, rhs, .. } => {
            check_arith_operands(lhs, col, mi, cx, params, sink);
            check_arith_operands(rhs, col, mi, cx, params, sink);
        }
        AssignRhs::Value(v) => {
            check_value(v, Some(mi), cx, params, sink);
            match v {
                Value::Path(p) => {
                    if let Some(t) = resolve_quiet(p, mi, cx) {
                        if terminal_family(&t) != Family::Numeric {
                            if let Some(seg) = p.segments.last() {
                                sink.error(
                                    code::ARITH_OPERAND,
                                    seg.span,
                                    format!(
                                        "{} is not numeric — an arithmetic update expression \
                                         takes int/float/decimal operands",
                                        terminal_name(&t)
                                    ),
                                );
                            }
                        }
                    }
                }
                // A literal carries no span, so a non-numeric one (`total + "x"`) is
                // anchored at the assignment target, like the assign-type error.
                Value::Lit(l) => {
                    if matches!(lit_family(l), Some(f) if f != Family::Numeric) {
                        sink.error(
                            code::ARITH_OPERAND,
                            col.span,
                            "a non-numeric literal in an arithmetic update expression".to_string(),
                        );
                    }
                }
                Value::Param(_) | Value::Func(_) => {}
            }
        }
    }
}

/// The column a param maps onto, for annotation agreement .
pub enum Mapped<'a> {
    Scalar(Primitive),
    Relation(&'a str),
}

/// An explicit param annotation must agree with the column it maps onto. A
/// relation param may be typed as its target model *or* as its key (`Id`/`Uuid`);
/// a scalar param must match the column's family. Loose on purpose (family, not
/// exact primitive) so `Uuid`↔`Id` and the like don't spuriously conflict.
pub fn check_param_type(ann: &TypeExpr, mapped: Mapped, sink: &mut Sink) {
    match (&ann.base, mapped) {
        (BaseType::Primitive(pann), Mapped::Scalar(pcol)) => {
            if prim_family(*pann) != prim_family(pcol) {
                sink.error(
                    code::PARAM_TYPE,
                    ann.span,
                    format!(
                        "param typed `{}` maps to a `{}` column",
                        prim_name(*pann),
                        prim_name(pcol)
                    ),
                );
            }
        }
        (BaseType::Primitive(pann), Mapped::Relation(target)) => {
            if !matches!(pann, Primitive::Id | Primitive::Uuid) {
                sink.error(
                    code::PARAM_TYPE,
                    ann.span,
                    format!(
                        "param typed `{}` binds relation `{target}`; use the model type or a key (`Id`)",
                        prim_name(*pann)
                    ),
                );
            }
        }
        (BaseType::Model(m), Mapped::Relation(target)) => {
            if m.node != target {
                sink.error(
                    code::PARAM_TYPE,
                    m.span,
                    format!("param typed `{}` binds a relation to `{target}`", m.node),
                );
            }
        }
        // The parser rejects `raw(…)` outside a model field type, so an opaque
        // annotation never reaches here.
        (BaseType::Raw(_), _) => {}
        (BaseType::Model(m), Mapped::Scalar(pcol)) => sink.error(
            code::PARAM_TYPE,
            m.span,
            format!(
                "param typed `{}` (a model) maps to a `{}` column",
                m.node,
                prim_name(pcol)
            ),
        ),
    }
}

fn family_name(f: Family) -> &'static str {
    match f {
        Family::Textual => "text",
        Family::Numeric => "numeric",
        Family::Bool => "bool",
        Family::Json => "json",
        Family::Key => "relation-key",
    }
}

fn op_sym(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::Like => "~",
        Op::In => "in",
        Op::Has => "has",
    }
}

/// Resolve `path` from model `start`. Reports the first failure and returns `None`.
pub fn resolve_path(path: &Path, start: usize, cx: &Cx, sink: &mut Sink) -> Option<Terminal> {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let Some(mem) = cx.model(cur).member(&seg.node) else {
            sink.error(
                code::UNKNOWN_FIELD,
                seg.span,
                format!("`{}` has no field `{}`", cx.model(cur).name, seg.node),
            );
            return None;
        };
        let last = i + 1 == n;
        match &mem.kind {
            MemberKind::Scalar { ty, raw_type, .. } => {
                if last {
                    return Some(match raw_type {
                        Some(spec) => Terminal::Opaque(spec.render()),
                        None => Terminal::Scalar(*ty),
                    });
                }
                sink.error(
                    code::TRAVERSE_SCALAR,
                    seg.span,
                    format!("cannot traverse into scalar column `{}`", seg.node),
                );
                return None;
            }
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return Some(Terminal::Relation(target.clone()));
                }
                // Target existence is validated during model checking; if it is
                // missing, that error already fired — just stop here.
                cur = cx.find(target)?;
            }
        }
    }
    None
}

/// Check one predicate against an optional model context. `params` is the set of
/// legal `$`-parameter names (besides `$ctx`, always allowed). When `model`
/// is `None` (a named filter checked at its declaration, without a caller), column
/// paths are not bound to a model — only params, filter calls, and functions are
/// checked; the body's columns resolve later at each call site (see below).
pub fn check_predicate(
    pred: &Predicate,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    check_predicate_in(pred, model, cx, params, &mut Vec::new(), sink);
}

/// `col <op> value`. When the left column is enum-typed, the right operand is a variant
/// (or a param), not a column — membership-checked instead of resolved as a path, which
/// would misread the variant as an unknown field.
fn check_cmp(
    path: &Path,
    op: Op,
    value: &Value,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let mut handled = false;
    if let Some(mi) = model {
        resolve_path(path, mi, cx, sink);
        if let Some(en) = cx.terminal_enum(path, mi) {
            // Ordered comparison is numeric-only: allowed on an int enum, rejected on a
            // string enum (its values have no order).
            if matches!(op, Op::Gt | Op::Lt | Op::Ge | Op::Le) && !en.is_int() {
                sink.error(
                    code::ENUM_ORDERED_OP,
                    path.segments.last().map_or(en.span, |s| s.span),
                    format!(
                        "`{}` is a string enum; ordered comparison is only valid on a \
                         numeric enum",
                        en.name
                    ),
                );
            }
            handled = check_enum_operand(value, en, params, sink);
        }
    }
    if !handled {
        check_value(value, model, cx, params, sink);
        // Operand typing runs after both sides' name errors are reported, and is silent
        // when either side failed to resolve.
        if let Some(mi) = model {
            check_cmp_types(path, op, value, mi, cx, sink);
        }
    }
}

/// `col in (…)`. Against an enum column each bare element is a variant — membership-
/// checked (E0154) instead of resolved as a column path.
fn check_in_list(
    path: &Path,
    values: &[Value],
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let en = model.and_then(|mi| {
        resolve_path(path, mi, cx, sink);
        cx.terminal_enum(path, mi)
    });
    for value in values {
        if en.is_some_and(|en| check_enum_operand(value, en, params, sink)) {
            continue;
        }
        check_value(value, model, cx, params, sink);
        if let Some(mi) = model {
            check_in_element_type(path, value, mi, cx, sink);
        }
    }
}

/// `filter(args…)`. On matching arity the filter's body is resolved against the call-site
/// model, so its column paths (`address.city = …`) are checked against the model the query
/// actually runs on — a filter has no model of its own. The arguments themselves are
/// values in the *caller's* param scope.
fn check_filter_call(
    name: &Ident,
    args: &[Value],
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    in_filters: &mut Vec<String>,
    sink: &mut Sink,
) {
    match cx.filters.get(&name.node) {
        None => sink.error(
            code::UNKNOWN_FILTER,
            name.span,
            format!("unknown filter `{}`", name.node),
        ),
        Some(def) if def.params.len() != args.len() => sink.error(
            code::FILTER_ARITY,
            name.span,
            format!(
                "filter `{}` takes {} argument(s), got {}",
                name.node,
                def.params.len(),
                args.len()
            ),
        ),
        Some(def) => resolve_filter_body(def, model, cx, in_filters, sink),
    }
    for v in args {
        check_value(v, model, cx, params, sink);
    }
}

/// Inner walker carrying `in_filters`, the stack of named filters currently being
/// expanded, so a filter that (directly or transitively) calls itself terminates
/// instead of recursing forever.
fn check_predicate_in(
    pred: &Predicate,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    in_filters: &mut Vec<String>,
    sink: &mut Sink,
) {
    match pred {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            check_predicate_in(a, model, cx, params, in_filters, sink);
            check_predicate_in(b, model, cx, params, in_filters, sink);
        }
        Predicate::Not(p) => check_predicate_in(p, model, cx, params, in_filters, sink),
        Predicate::Cmp { path, op, value } => {
            check_cmp(path, *op, value, model, cx, params, sink);
        }
        Predicate::InList { path, values } => {
            check_in_list(path, values, model, cx, params, sink);
        }
        Predicate::Bare(path) => {
            // A bare atom is a bool column or a zero-arg named-filter reference.
            if path.segments.len() == 1 {
                if let Some(def) = cx.filters.get(&path.segments[0].node) {
                    resolve_filter_body(def, model, cx, in_filters, sink);
                    return;
                }
            }
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
            }
        }
        Predicate::FilterCall { name, args } => {
            check_filter_call(name, args, model, cx, params, in_filters, sink);
        }
        Predicate::Raw(raw) => {
            check_raw_params(raw, params, sink);
            if let Some(mi) = model {
                if let Some(sd) = &cx.model(mi).soft_delete {
                    sink.warn(
                        code::RAW_SOFT_DELETE_GAP,
                        raw.span,
                        format!(
                            "raw SQL on soft-delete model `{}`: engine can't verify the `{}` tombstone filter — confirm it",
                            cx.model(mi).name, sd.field
                        ),
                    );
                }
            }
        }
    }
}

/// Re-resolve a named filter's body against the call-site `model`. The filter's
/// *own* params are the legal `$`-set inside its body (a filter param is referenced
/// as `$c`, same `$`-means-bound rule as everywhere else).
/// `in_filters` guards against a filter that expands to itself. With no call-site
/// model (`model` is `None`, e.g. a filter reached while checking another filter's
/// declaration) there is nothing to resolve columns against, so this is a no-op.
fn resolve_filter_body(
    def: &NamedFilter,
    model: Option<usize>,
    cx: &Cx,
    in_filters: &mut Vec<String>,
    sink: &mut Sink,
) {
    if model.is_none() || in_filters.iter().any(|n| n == &def.name.node) {
        return;
    }
    let fparams: Vec<String> = def.params.iter().map(|p| p.name.node.clone()).collect();
    in_filters.push(def.name.node.clone());
    check_predicate_in(&def.pred, model, cx, &fparams, in_filters, sink);
    in_filters.pop();
}

pub fn check_value(
    value: &Value,
    model: Option<usize>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    match value {
        Value::Param(pr) => check_param_ref(pr, params, sink),
        Value::Path(path) => {
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
            }
        }
        Value::Lit(_) => {}
        Value::Func(f) => check_func(f, model, cx, params, sink),
    }
}

pub fn check_param_ref(pr: &ParamRef, params: &[String], sink: &mut Sink) {
    // `$ctx` is the caller-supplied request context . It must be referenced
    // as exactly `$ctx.<field>` (one segment — the fields are flat). Its *type* is
    // not declared: it is inferred per callable from the column each use compares
    // against, and checked for cross-callable coherence (see `ctx.rs`).
    if pr.name.node == "ctx" {
        if pr.path.len() != 1 {
            let span = pr.path.last().map_or(pr.name.span, |s| s.span);
            sink.error(
                code::CTX_BAD_PATH,
                span,
                "`$ctx` takes exactly one field (e.g. `$ctx.org`)",
            );
        }
        return;
    }
    if !params.iter().any(|p| p == &pr.name.node) {
        sink.error(
            code::UNKNOWN_PARAM,
            pr.name.span,
            format!("unknown parameter `${}`", pr.name.node),
        );
    }
}

pub fn check_func(f: &FuncCall, model: Option<usize>, cx: &Cx, params: &[String], sink: &mut Sink) {
    if !KNOWN_FUNCS.contains(&f.name.node.as_str()) {
        sink.error(
            code::UNKNOWN_FUNC,
            f.name.span,
            format!(
                "unknown function `{}` (available: {})",
                f.name.node,
                KNOWN_FUNCS.join(", ")
            ),
        );
    }
    for a in &f.args {
        check_value(a, model, cx, params, sink);
    }
}

/// A field/param default: only its function (e.g. `now()`) needs checking.
pub fn check_default(dv: &DefaultVal, sink: &mut Sink) {
    if let DefaultVal::Func(f) = dv {
        if !KNOWN_FUNCS.contains(&f.name.node.as_str()) {
            sink.error(
                code::UNKNOWN_FUNC,
                f.name.span,
                format!(
                    "unknown function `{}` (available: {})",
                    f.name.node,
                    KNOWN_FUNCS.join(", ")
                ),
            );
        }
    }
}

fn check_raw_params(raw: &RawSql, params: &[String], sink: &mut Sink) {
    for part in &raw.parts {
        if let RawPart::Param(pr) = part {
            check_param_ref(pr, params, sink);
        }
    }
}

pub fn check_sort_term(t: &SortTerm, model: usize, cx: &Cx, sink: &mut Sink) {
    if let Some(term) = resolve_path(&t.path, model, cx, sink) {
        reject_opaque(&term, &t.path, "sort", sink);
    }
}

/// An opaque `raw(…)` column has no type the engine can compare or order by, so it is
/// never a filter / sort / group / aggregate operand (`E0271`). The read side stays open
/// through the `raw` leaf — a raw predicate term or a raw shape value (`ST_Area(geom)`),
/// where the SQL author owns the semantics.
pub fn reject_opaque(term: &Terminal, path: &Path, what: &str, sink: &mut Sink) -> bool {
    let Terminal::Opaque(ty) = term else {
        return false;
    };
    let Some(seg) = path.segments.last() else {
        return false;
    };
    sink.error_note(
        code::OPAQUE_OPERAND,
        seg.span,
        format!("cannot {what} by `{}` — a {ty} column is opaque", seg.node),
        "the engine does not model this type; reach it with a `raw` predicate term or a raw shape value",
    );
    true
}

/// Resolve a relation's custom `on:` join predicate.
/// Unlike every other predicate this one spans *two* tables — the FK-holding
/// model `near` and its target `far` — and refers to columns table-qualified
/// (`orders.user_ref = users.legacy_id`), for legacy keys that don't follow the
/// `<field>_id` convention. Each column path must be `<table>.<column>` naming one
/// of the two tables in scope and a real column on it, so the join stays inside the
/// guarantee (the engine still understands and types it). A join is static
/// structure, so request `$`-params, filter calls, and `^` back-references have no
/// meaning here and are rejected; literals (a constant discriminator) are fine.
pub fn check_relation_on(pred: &Predicate, near: usize, far: usize, cx: &Cx, sink: &mut Sink) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            check_relation_on(a, near, far, cx, sink);
            check_relation_on(b, near, far, cx, sink);
        }
        Predicate::Not(p) => check_relation_on(p, near, far, cx, sink),
        Predicate::Cmp { path, value, .. } => {
            resolve_join_path(path, near, far, cx, sink);
            check_join_value(value, near, far, cx, sink);
        }
        Predicate::InList { path, values } => {
            resolve_join_path(path, near, far, cx, sink);
            for v in values {
                check_join_value(v, near, far, cx, sink);
            }
        }
        Predicate::Bare(path) => resolve_join_path(path, near, far, cx, sink),
        Predicate::FilterCall { name, .. } => sink.error(
            code::JOIN_FORM,
            name.span,
            "named filters aren't allowed in a custom `on:` join condition",
        ),
        // A raw join fragment is an escape hatch the engine can't resolve — leave it.
        Predicate::Raw(_) => {}
    }
}

/// One value in a custom `on:` join: a column resolves table-qualified, a literal
/// (a constant discriminator) is fine, and anything request-bound is rejected —
/// a join is static structure.
fn check_join_value(value: &Value, near: usize, far: usize, cx: &Cx, sink: &mut Sink) {
    match value {
        Value::Path(p) => resolve_join_path(p, near, far, cx, sink),
        Value::Lit(_) => {}
        Value::Param(pr) => sink.error(
            code::JOIN_FORM,
            pr.name.span,
            "a custom join is static structure — `$` params aren't bound in an `on:` condition",
        ),
        Value::Func(f) => sink.error(
            code::JOIN_FORM,
            f.name.span,
            "a custom join is static structure — functions aren't allowed in an `on:` condition",
        ),
    }
}

/// Resolve one table-qualified column path (`orders.user_ref`) in a custom join.
/// Must be exactly two segments: a table naming `near` or `far`, then a physical
/// column on that model. Self-ref joins (`near == far`) resolve against the one
/// model on either side; distinguishing the two logical sides is a codegen alias
/// concern, not a resolution one.
fn resolve_join_path(path: &Path, near: usize, far: usize, cx: &Cx, sink: &mut Sink) {
    let Some(last) = path.segments.last() else {
        return;
    };
    if path.segments.len() != 2 {
        sink.error(
            code::JOIN_FORM,
            last.span,
            "custom-join column must be table-qualified `<table>.<column>`",
        );
        return;
    }
    let table = &path.segments[0];
    let col = &path.segments[1];
    let mi = if table.node == cx.model(near).table {
        near
    } else if table.node == cx.model(far).table {
        far
    } else {
        sink.error(
            code::JOIN_TABLE,
            table.span,
            format!(
                "unknown table `{}` in join (expected `{}` or `{}`)",
                table.node,
                cx.model(near).table,
                cx.model(far).table
            ),
        );
        return;
    };
    if cx.model(mi).column(&col.node).is_none() {
        sink.error(
            code::UNKNOWN_FIELD,
            col.span,
            format!(
                "table `{}` has no column `{}`",
                cx.model(mi).table,
                col.node
            ),
        );
    }
}
