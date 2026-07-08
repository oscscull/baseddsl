//! `$ctx` inference + coherence .
//!
//! `$ctx` is the caller-supplied request context (auth.md). It is **per-request**,
//! so there is no single global context type; each callable *requires* exactly the
//! `$ctx.<field>`s it reads — directly in a `where`, through its target model's
//! `@scope`, through an expanded filter body, or in a `create`/`update` assign.
//!
//! A field's type is never declared: it is inferred from the column each use is
//! compared against (`org = $ctx.org` ⇒ `org` is the FK, so `$ctx.org` is that
//! model's key). Uses with no column to infer from (a literal, a raw block, a
//! guard arg) contribute nothing.
//!
//! The one global fact is **coherence**: every callable reads from the same bag
//! the caller builds per request, so a field *name* must mean one *type* across
//! the whole closed world. `check_coherence` enforces that (`CTX_CONFLICT`).

use based_ast::*;

use crate::ir::*;
use crate::resolve::{self, Cx, Terminal};

/// The `$ctx` a query requires: its own `where`s + its target model's `@scope`,
/// each field typed by inference. Deduped (one entry per distinct field+type).
pub fn collect_query(q: &Query, ti: usize, cx: &Cx) -> Vec<CtxReq> {
    let mut out = Vec::new();
    // `@scope` is injected into every query on the model (auth.md Handle 2), so a
    // scope that reads `$ctx` makes that field a requirement of every such query —
    // unless the query is `unscoped` , which drops the injection and the need.
    if q.unscoped.is_none() {
        if let Some(scope) = &cx.model(ti).scope {
            walk_pred(scope, ti, cx, &mut Vec::new(), &mut out);
        }
    }
    for clause in query_clauses(q) {
        if let Clause::Where(p) = clause {
            walk_pred(p, ti, cx, &mut Vec::new(), &mut out);
        }
    }
    // Joined *scoped* models : codegen injects a joined model's `@scope` into
    // its join `ON`, so a query that reaches another scoped tenant through a relation
    // must *also* require that model's `$ctx` field (else the injected `:ctx_<field>`
    // bind is unbound at runtime). `unscoped` drops the whole scope machinery, joins
    // included, so it collects none — mirroring the codegen `with_scope_inject(false)`.
    if q.unscoped.is_none() {
        collect_joined_scope(q, ti, cx, &mut out);
    }
    dedup(out)
}

/// Collect the `@scope` `$ctx` requirements of every scoped model a query *joins*
/// . The join sources are exactly codegen's: relation reaches in a `where` path,
/// the sort path (query `order`, else the model `@sort`), and the return shape's
/// `out = path` reaches. A `Nest { … }` shape sub-object lowers to a correlated subquery
/// carrying its own scoped `WHERE` , not an outer join, so it is deliberately not
/// walked — sema and codegen stay aligned on exactly which joins exist.
fn collect_joined_scope(q: &Query, ti: usize, cx: &Cx, out: &mut Vec<CtxReq>) {
    // `where` paths.
    for clause in query_clauses(q) {
        match clause {
            Clause::Where(p) => walk_pred_paths(p, ti, cx, out),
            Clause::Order(terms) => {
                for t in terms {
                    walk_path_scope(&t.path, ti, cx, out);
                }
            }
            _ => {}
        }
    }
    // Model `@sort` only applies when the query supplies no `order` (codegen's cascade).
    let has_query_order = query_clauses(q)
        .iter()
        .any(|c| matches!(c, Clause::Order(_)));
    if !has_query_order {
        for t in &cx.model(ti).sort {
            walk_path_scope(&t.path, ti, cx, out);
        }
    }
    // Return shape reaches.
    if let Some(body) = cx.shape_bodies.get(&q.ret.ty.node) {
        walk_shape_scope(body, ti, cx, out);
    }
}

/// Walk every column path in a predicate, recording joined-model scope for each
/// relation reach. Filter calls expand against the call site , guarded against
/// self-reference. (`walk_pred` above collects *direct* `$ctx` uses; this collects
/// the *joined-model* scope those same paths traverse — a separate concern.)
fn walk_pred_paths(pred: &Predicate, model: usize, cx: &Cx, out: &mut Vec<CtxReq>) {
    walk_pred_paths_in(pred, model, cx, &mut Vec::new(), out);
}

fn walk_pred_paths_in(
    pred: &Predicate,
    model: usize,
    cx: &Cx,
    in_filters: &mut Vec<String>,
    out: &mut Vec<CtxReq>,
) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            walk_pred_paths_in(a, model, cx, in_filters, out);
            walk_pred_paths_in(b, model, cx, in_filters, out);
        }
        Predicate::Not(p) => walk_pred_paths_in(p, model, cx, in_filters, out),
        Predicate::Cmp { path, value, .. } => {
            walk_path_scope(path, model, cx, out);
            if let Value::Path(p) = value {
                walk_path_scope(p, model, cx, out);
            }
        }
        Predicate::Bare(path) => {
            if path.segments.len() == 1 {
                if let Some(def) = cx.filters.get(&path.segments[0].node) {
                    walk_filter_paths(def, model, cx, in_filters, out);
                    return;
                }
            }
            walk_path_scope(path, model, cx, out);
        }
        Predicate::FilterCall { name, .. } => {
            if let Some(def) = cx.filters.get(&name.node) {
                walk_filter_paths(def, model, cx, in_filters, out);
            }
        }
        Predicate::Raw(_) => {}
    }
}

fn walk_filter_paths(
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
    walk_pred_paths_in(&def.pred, model, cx, in_filters, out);
    in_filters.pop();
}

/// Walk a return shape body: an `out = path` reach may join, a `Bare` never does
/// (single-segment), a `Nest` lowers to a subquery in codegen (no outer join → not walked, D34).
fn walk_shape_scope(body: &[ShapeField], model: usize, cx: &Cx, out: &mut Vec<CtxReq>) {
    for f in body {
        if let ShapeField::Rename {
            value: ShapeValue::Path(p),
            ..
        } = f
        {
            walk_path_scope(p, model, cx, out);
        }
    }
}

/// Walk a dotted path from `start`, and at every *intermediate* model entered through
/// a relation hop (a join in codegen), collect that model's `@scope` `$ctx` fields.
/// The final segment is a terminal column/FK, not a join, so it is skipped. Mirrors
/// codegen's `Select::resolve` (a join per non-last relation hop).
fn walk_path_scope(path: &Path, start: usize, cx: &Cx, out: &mut Vec<CtxReq>) {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let Some(mem) = cx.model(cur).member(&seg.node) else {
            return; // sema already reported the unknown field
        };
        if i + 1 == n {
            return; // terminal segment — a column/FK, never a join
        }
        match &mem.kind {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                let Some(mi) = cx.find(target) else { return };
                // The join into `target` carries `target`'s `@scope` .
                if let Some(scope) = &cx.model(mi).scope {
                    walk_pred(scope, mi, cx, &mut Vec::new(), out);
                }
                cur = mi;
            }
            MemberKind::Scalar { .. } => return, // can't traverse a scalar
        }
    }
}

/// The `$ctx` a mutation requires: each write statement's `where` + its model's
/// `@scope` (update/delete/restore inject it, D12) + `create`/`update` assigns, plus
///  any scoped model a write `where` or the create's declared-shape re-select
/// *joins*. `ret_shape`/`ret_model` describe that re-select's projection.
pub fn collect_mutation(
    m: &Mutation,
    ret_shape: Option<&str>,
    ret_model: &str,
    cx: &Cx,
) -> Vec<CtxReq> {
    let mut out = Vec::new();
    // `unscoped`  drops both the injected write guard and the create-time auto-set,
    // so a scoped model contributes no `$ctx` requirement to an unscoped mutation.
    let unscoped = m.unscoped.is_some();
    for stmt in &m.body {
        walk_write(stmt, cx, unscoped, &mut out);
    }
    // Joined-model scope , unless `unscoped` (which drops all scope handling).
    if !unscoped {
        for stmt in &m.body {
            collect_write_joined_scope(stmt, cx, &mut out);
        }
        // The declared-shape re-select  projects `ret_shape` from `ret_model`,
        // so its relation reaches join scoped models exactly like a query's shape.
        if let (Some(name), Some(mi)) = (ret_shape, cx.find(ret_model)) {
            if let Some(body) = cx.shape_bodies.get(name) {
                walk_shape_scope(body, mi, cx, &mut out);
            }
        }
    }
    dedup(out)
}

/// A write's `where` paths reach through relations (update/delete/restore); each
/// non-terminal hop into a scoped model is a scope-injected join . A `create`
/// has no `where`; a `tx` recurses.
fn collect_write_joined_scope(stmt: &WriteStmt, cx: &Cx, out: &mut Vec<CtxReq>) {
    match stmt {
        WriteStmt::Update { model, where_, .. }
        | WriteStmt::Delete { model, where_ }
        | WriteStmt::HardDelete { model, where_ }
        | WriteStmt::Restore { model, where_ } => {
            if let Some(mi) = cx.find(&model.node) {
                walk_pred_paths(where_, mi, cx, out);
            }
        }
        WriteStmt::Tx(inner) => {
            for s in inner {
                collect_write_joined_scope(s, cx, out);
            }
        }
        WriteStmt::Create { .. } | WriteStmt::Raw(_) => {}
    }
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
        QueryBody::Bare => &[],
    }
}

fn walk_write(stmt: &WriteStmt, cx: &Cx, unscoped: bool, out: &mut Vec<CtxReq>) {
    match stmt {
        WriteStmt::Create { model, assigns } => {
            if let Some(mi) = cx.find(&model.node) {
                // A create on a scoped model auto-sets the scope column from `$ctx`
                // , so it *requires* that field — the create-time twin of the
                // read/write injection. An explicit assign may also set one from ctx.
                scope_ctx(mi, cx, unscoped, out);
                for a in assigns {
                    record_assign(a, mi, cx, out);
                }
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(mi) = cx.find(&model.node) {
                scope_ctx(mi, cx, unscoped, out);
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
                scope_ctx(mi, cx, unscoped, out);
                walk_pred(where_, mi, cx, &mut Vec::new(), out);
            }
        }
        WriteStmt::Tx(inner) => {
            for s in inner {
                walk_write(s, cx, unscoped, out);
            }
        }
        WriteStmt::Raw(_) => {} // opaque — no column to infer against
    }
}

fn scope_ctx(mi: usize, cx: &Cx, unscoped: bool, out: &mut Vec<CtxReq>) {
    if unscoped {
        return;
    }
    if let Some(scope) = &cx.model(mi).scope {
        walk_pred(scope, mi, cx, &mut Vec::new(), out);
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
        Predicate::Cmp { path, value, .. } => {
            if let Value::Param(pr) = value {
                if let Some(field) = ctx_field(pr) {
                    if let Some(term) = resolve::resolve_path(path, model, cx, &mut Sink::default())
                    {
                        out.push(CtxReq {
                            field,
                            ty: term_to_ctx(&term),
                            span: pr.path[0].span,
                        });
                    }
                }
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
    let Value::Param(pr) = &a.value else { return };
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
        Primitive::Int => 1, // numeric
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
    }
}
