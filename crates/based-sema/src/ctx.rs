//! `$ctx` inference + coherence .
//!
//! `$ctx` is the caller-supplied request context. It is **per-request**,
//! so there is no single global context type; each callable *requires* exactly the
//! `$ctx.<field>`s it reads — directly in a `where`, through an expanded filter
//! body, in a `create`/`update` assign, or through the scope terms it injects
//! (the caller passes those in as `scope_reqs`, resolved from the callable's
//! *chosen* `@scope` alternative by `scope::inject_ctx_reqs`, so the requirement
//! set always matches the `:ctx_<field>` binds codegen emits).
//!
//! A directly-used field's type is never declared: it is inferred from the column
//! each use is compared against (`org = $ctx.org` ⇒ `org` is the FK, so `$ctx.org`
//! is that model's key). A scope field's type comes from its `scope` decl. Uses
//! with no column to infer from (a literal, a raw block, a guard arg) contribute
//! nothing.
//!
//! The one global fact is **coherence**: every callable reads from the same bag
//! the caller builds per request, so a field *name* must mean one *type* across
//! the whole closed world. `check_coherence` enforces that (`CTX_CONFLICT`).

use based_ast::*;

use crate::ir::*;
use crate::resolve::{self, Cx, Terminal};

/// The `$ctx` a query requires: the scope terms it injects (`scope_reqs` — the
/// caller resolves them from the query's chosen `@scope` alternative per touched
/// model, `scope::inject_ctx_reqs`, so the requirement matches the `:ctx_<field>`
/// binds codegen emits) plus its own `where`s, each field typed by inference.
/// Deduped (one entry per distinct field+type).
pub fn collect_query(q: &Query, ti: usize, cx: &Cx, scope_reqs: Vec<CtxReq>) -> Vec<CtxReq> {
    let mut out = scope_reqs;
    for clause in query_clauses(q) {
        if let Clause::Where(p) = clause {
            walk_pred(p, ti, cx, &mut Vec::new(), &mut out);
        }
    }
    dedup(out)
}

/// The `$ctx` a mutation requires: the scope terms it injects (`scope_reqs`, from
/// the mutation's chosen `@scope` alternative per touched model — write guards,
/// the create-time auto-set, and the re-select's joined scopes alike) plus each
/// write statement's `where` and `create`/`update` assigns.
pub fn collect_mutation(m: &Mutation, cx: &Cx, scope_reqs: Vec<CtxReq>) -> Vec<CtxReq> {
    let mut out = scope_reqs;
    for stmt in &m.body {
        walk_write(stmt, cx, &mut out);
    }
    dedup(out)
}

/// Closed-world coherence : a `$ctx.<field>` must carry the same type
/// everywhere, since every callable reads the one context bag the caller builds.
/// Reports `CTX_CONFLICT` at the first use whose type disagrees with an earlier one.
pub fn check_coherence(queries: &[RQuery], mutations: &[RMutation], sink: &mut Sink) {
    // field -> the first (type, human name) seen. Iteration order is the callable
    // declaration order, so the diagnostic points at the later, conflicting use.
    let mut seen: std::collections::HashMap<&str, (&CtxField, String)> =
        std::collections::HashMap::new();
    let all = queries
        .iter()
        .flat_map(|q| &q.ctx_requires)
        .chain(mutations.iter().flat_map(|m| &m.ctx_requires));
    for req in all {
        match seen.get(req.field.as_str()) {
            Some((prev_ty, prev_name)) if !compatible(prev_ty, &req.ty) => sink.error_note(
                code::CTX_CONFLICT,
                req.span,
                format!(
                    "`$ctx.{}` is used as {} here but {} elsewhere",
                    req.field,
                    ty_name(&req.ty),
                    prev_name
                ),
                "the caller supplies one request context; a field must have one type",
            ),
            Some(_) => {}
            None => {
                seen.insert(req.field.as_str(), (&req.ty, ty_name(&req.ty)));
            }
        }
    }
}

// ---------- predicate + write walkers --------------------------------------

fn query_clauses(q: &Query) -> &[Clause] {
    match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        // A raw body has no clauses; `${ctx.…}` inside it is rejected in sema
        // (no type source), so it contributes no requirement here either.
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    }
}

fn walk_write(stmt: &WriteStmt, cx: &Cx, out: &mut Vec<CtxReq>) {
    match stmt {
        WriteStmt::Create {
            model,
            assigns,
            conflict,
        } => {
            if let Some(mi) = cx.find(&model.node) {
                for a in assigns {
                    record_assign(a, mi, cx, out);
                }
                if let Some(oc) = conflict {
                    for a in &oc.update {
                        record_assign(a, mi, cx, out);
                    }
                }
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(mi) = cx.find(&model.node) {
                walk_pred(where_, mi, cx, &mut Vec::new(), out);
                for a in assigns {
                    record_assign(a, mi, cx, out);
                }
            }
        }
        WriteStmt::Delete { model, where_ }
        | WriteStmt::HardDelete { model, where_ }
        | WriteStmt::Restore { model, where_ } => {
            if let Some(mi) = cx.find(&model.node) {
                walk_pred(where_, mi, cx, &mut Vec::new(), out);
            }
        }
        WriteStmt::Tx(inner) => {
            for s in inner {
                walk_write(s, cx, out);
            }
        }
        WriteStmt::Raw(_) => {} // opaque — no column to infer against
    }
}

/// Walk a predicate against `model`, recording every `$ctx.<field>` compared to a
/// resolvable column. Filter calls expand against the call-site model, guarded by
/// `in_filters` so a self-referential filter terminates .
fn walk_pred(
    pred: &Predicate,
    model: usize,
    cx: &Cx,
    in_filters: &mut Vec<String>,
    out: &mut Vec<CtxReq>,
) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            walk_pred(a, model, cx, in_filters, out);
            walk_pred(b, model, cx, in_filters, out);
        }
        Predicate::Not(p) => walk_pred(p, model, cx, in_filters, out),
        Predicate::Cmp { path, value, .. } => record_ctx_use(path, value, model, cx, out),
        Predicate::InList { path, values } => {
            for v in values {
                record_ctx_use(path, v, model, cx, out);
            }
        }
        Predicate::Bare(path) => {
            if path.segments.len() == 1 {
                if let Some(def) = cx.filters.get(&path.segments[0].node) {
                    expand_filter(def, model, cx, in_filters, out);
                }
            }
        }
        Predicate::FilterCall { name, .. } => {
            // The args (which may themselves be `$ctx`) bind to filter params whose
            // column usage is not tracked .
            // The body's *direct* `$ctx` uses are inferred against the call site.
            if let Some(def) = cx.filters.get(&name.node) {
                expand_filter(def, model, cx, in_filters, out);
            }
        }
        Predicate::Raw(_) => {}
    }
}

/// Record one `$ctx.<field>` value compared to `path`, typed by the column.
fn record_ctx_use(path: &Path, value: &Value, model: usize, cx: &Cx, out: &mut Vec<CtxReq>) {
    let Value::Param(pr) = value else { return };
    let Some(field) = ctx_field(pr) else { return };
    if let Some(term) = resolve::resolve_path(path, model, cx, &mut Sink::default()) {
        out.push(CtxReq {
            field,
            ty: term_to_ctx(&term),
            span: pr.path[0].span,
        });
    }
}

fn expand_filter(
    def: &NamedFilter,
    model: usize,
    cx: &Cx,
    in_filters: &mut Vec<String>,
    out: &mut Vec<CtxReq>,
) {
    if in_filters.iter().any(|n| n == &def.name.node) {
        return;
    }
    in_filters.push(def.name.node.clone());
    walk_pred(&def.pred, model, cx, in_filters, out);
    in_filters.pop();
}

/// `col = $ctx.field` in a `create`/`update`: infer the field's type from the
/// assigned column (a scalar's primitive, or a forward relation's target key).
fn record_assign(a: &Assign, mi: usize, cx: &Cx, out: &mut Vec<CtxReq>) {
    let Some(Value::Param(pr)) = a.value.as_value() else {
        return;
    };
    let Some(field) = ctx_field(pr) else { return };
    let Some(member) = cx.model(mi).member(&a.col.node) else {
        return;
    };
    let ty = match &member.kind {
        MemberKind::Scalar { ty, .. } => CtxField::Scalar(*ty),
        MemberKind::Forward { target, .. } => CtxField::Relation(target.clone()),
        MemberKind::Inverse { .. } => return, // not an assignable column
    };
    out.push(CtxReq {
        field,
        ty,
        span: pr.path[0].span,
    });
}

// ---------- helpers --------------------------------------------------------

/// The field name of a well-formed `$ctx.<field>` (exactly one segment). A
/// malformed path is reported by `resolve::check_param_ref`; here it just yields
/// nothing to infer.
fn ctx_field(pr: &ParamRef) -> Option<String> {
    (pr.name.node == "ctx" && pr.path.len() == 1).then(|| pr.path[0].node.clone())
}

fn term_to_ctx(t: &Terminal) -> CtxField {
    match t {
        Terminal::Scalar(p) => CtxField::Scalar(*p),
        Terminal::Relation(m) => CtxField::Relation(m.clone()),
        // Unreachable: an opaque column is never a `$ctx` comparison operand (E0271).
        Terminal::Opaque(_) => CtxField::Scalar(Primitive::Text),
    }
}

/// Two inferred types for the same field agree when they are the same primitive
/// family, or relations to the same model. Coarse on primitives (family, not exact
/// type) to match the operand checker, but strict on relations — `$ctx.org` should
/// not be an `Org` key in one place and a `User` key in another.
fn compatible(a: &CtxField, b: &CtxField) -> bool {
    match (a, b) {
        (CtxField::Scalar(x), CtxField::Scalar(y)) => prim_family(*x) == prim_family(*y),
        (CtxField::Relation(x), CtxField::Relation(y)) => x == y,
        _ => false,
    }
}

fn ty_name(t: &CtxField) -> String {
    match t {
        CtxField::Scalar(p) => format!("`{}`", prim_name(*p)),
        CtxField::Relation(m) => format!("a `{m}` key"),
    }
}

/// Drop exact duplicates (same field + same type) while keeping distinct types for
/// the same field — an intra-callable clash must survive to the coherence pass.
fn dedup(reqs: Vec<CtxReq>) -> Vec<CtxReq> {
    let mut out: Vec<CtxReq> = Vec::new();
    for r in reqs {
        if !out
            .iter()
            .any(|e| e.field == r.field && compatible(&e.ty, &r.ty))
        {
            out.push(r);
        }
    }
    out
}

// Primitive family/name mirror the operand checker's coarse buckets (resolve.rs).
// Kept local so `$ctx` typing doesn't couple to that module's private helpers.
fn prim_family(ty: Primitive) -> u8 {
    match ty {
        Primitive::Text
        | Primitive::Uuid
        | Primitive::Id
        | Primitive::Timestamp
        | Primitive::Date => 0, // textual (string-writable + orderable, D1)
        Primitive::Int | Primitive::Float | Primitive::Decimal { .. } => 1, // numeric
        Primitive::Bool => 2,
        Primitive::Json => 3,
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
