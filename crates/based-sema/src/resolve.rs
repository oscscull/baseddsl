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
        Primitive::Int => Family::Numeric,
        Primitive::Bool => Family::Bool,
        Primitive::Json => Family::Json,
    }
}

fn terminal_family(t: &Terminal) -> Family {
    match t {
        Terminal::Scalar(p) => prim_family(*p),
        Terminal::Relation(_) => Family::Key,
    }
}

/// Human-facing name for a resolved operand, for diagnostics.
fn terminal_name(t: &Terminal) -> String {
    match t {
        Terminal::Scalar(p) => format!("`{}`", prim_name(*p)),
        Terminal::Relation(m) => format!("relation `{m}`"),
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
    }
}

fn lit_family(l: &Literal) -> Option<Family> {
    match l {
        Literal::Str(_) => Some(Family::Textual),
        Literal::Int(_) | Literal::Float(_) => Some(Family::Numeric),
        Literal::Bool(_) => Some(Family::Bool),
        Literal::Null => None, // null is compatible with any operand
    }
}

/// Are two operand families comparable with `=`/`!=` (and, for orderable ones, the
/// relational operators)? Json matches anything; a relation key accepts either a
/// uuid string or an integer key.
fn compatible(a: Family, b: Family) -> bool {
    use Family::*;
    if a == Json || b == Json {
        return true;
    }
    match (a, b) {
        (Key, Textual) | (Textual, Key) | (Key, Numeric) | (Numeric, Key) => true,
        _ => a == b,
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
        // here; a function's return type is not modelled yet. A `^` back-reference is
        // only valid in a write assign (not a predicate), reported there.
        Value::Param(_) | Value::Func(_) | Value::Back(_) => return,
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

/// The family a column accepts on assignment: a scalar's primitive family, or a
/// forward relation FK's key. An inverse edge owns no column, so it can't be
/// assigned — `None` skips the check (the misuse, if any, is out of scope here).
fn member_family(kind: &MemberKind) -> Option<Family> {
    match kind {
        MemberKind::Scalar { ty, .. } => Some(prim_family(*ty)),
        MemberKind::Forward { .. } => Some(Family::Key),
        MemberKind::Inverse { .. } => None,
    }
}

/// Type-check one `create`/`update` assignment: the value's family must agree with
/// the target column's — the write-side twin of the `=` compatibility rule
/// (`check_cmp_types`). A literal or another column is family-checked; a `^`
/// back-reference is typed by the field it reads on the preceding create (`back`,
/// the same model `check::check_back` resolved it against). Params (typed at their
/// declaration / `$ctx` inferred) and functions (return type unmodelled) are
/// skipped, exactly as on the read side. Silent when a side fails to resolve — that
/// name error is already reported by the caller.
pub fn check_assign_type(
    target: &MemberKind,
    col: &Ident,
    value: &Value,
    mi: usize,
    back: Option<usize>,
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
        Value::Back(b) => match back.and_then(|bm| cx.model(bm).member(&b.field.node)) {
            Some(m) => match member_family(&m.kind) {
                Some(f) => f,
                None => return,
            },
            None => return, // scope / unknown-field error already reported
        },
        Value::Param(_) | Value::Func(_) => return,
    };
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
            MemberKind::Scalar { ty, .. } => {
                if last {
                    return Some(Terminal::Scalar(*ty));
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
            let mut handled = false;
            if let Some(mi) = model {
                resolve_path(path, mi, cx, sink);
                // When the left column is enum-typed, the right operand is a variant
                // (or a param), not a column — check membership instead of resolving it
                // as a path (which would misread the variant as an unknown field).
                if let Some(en) = cx.terminal_enum(path, mi) {
                    // Ordered comparison is numeric-only: allowed on an int enum,
                    // rejected on a string enum (its values have no order).
                    if matches!(op, Op::Gt | Op::Lt | Op::Ge | Op::Le) && !en.is_int() {
                        sink.error(
                            code::ENUM_ORDERED_OP,
                            path.segments.last().map(|s| s.span).unwrap_or(en.span),
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
                // Operand typing runs after both sides' name errors are reported, and
                // is silent when either side failed to resolve.
                if let Some(mi) = model {
                    check_cmp_types(path, *op, value, mi, cx, sink);
                }
            }
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
                // Arity is right: resolve the filter's body against the call-site
                // model, so its column paths (`address.city = …`) are checked
                // against the model the query actually runs on (a filter has no
                // model of its own).
                Some(def) => resolve_filter_body(def, model, cx, in_filters, sink),
            }
            // The arguments themselves are values in the *caller's* param scope.
            for v in args {
                check_value(v, model, cx, params, sink);
            }
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
        // A `^` back-reference is only meaningful in a `tx` write assign, where
        // `check::check_assign` resolves it against the preceding create. Reaching it
        // here (a predicate, a function argument, a query) is a misuse.
        Value::Back(b) => sink.error(
            code::BACKREF_SCOPE,
            b.span,
            "`^` back-reference is only valid in a `tx` write (e.g. `user = ^.id`)",
        ),
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
    resolve_path(&t.path, model, cx, sink);
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
                Value::Back(b) => sink.error(
                    code::JOIN_FORM,
                    b.span,
                    "`^` back-reference is only valid in a `tx` write, not a custom join",
                ),
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
