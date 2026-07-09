//! Named-scope resolution.
//!
//! A `scope Name (col: Type = $ctx.field, …)` decl is the one source of truth
//! for a standing row-visibility filter: the predicate form (a conjunction of
//! `col = $ctx.field`, `E0180`) and the scope column's — hence `$ctx.field`'s — type.
//! A model opts in with `@scope Name` (a set of alternatives); a callable
//! acknowledges with `scoped Name` or opts out with `unscoped(…)`.
//!
//! This module resolves the decls into [`RScope`], then attaches each model's
//! `@scope` refs: it checks the names exist (`E0183`) and the model carries each
//! scope's columns at a conforming type (`E0184`), and synthesizes the injected
//! predicate ([`RModel::scope`]) so codegen lowers scope exactly as before.

use based_ast::*;
use std::collections::HashMap;

use crate::ir::*;

/// Resolve every `scope` decl into an [`RScope`], reporting duplicates (`E0105`),
/// malformed terms (`E0180`), and unknown model types. Returns the resolved scopes
/// plus a name→index lookup.
pub fn resolve_decls(
    decls: &[&ScopeDecl],
    model_index: &HashMap<String, usize>,
    sink: &mut Sink,
) -> (Vec<RScope>, HashMap<String, usize>) {
    let mut scopes = Vec::new();
    let mut index = HashMap::new();
    for d in decls {
        if index.contains_key(&d.name.node) {
            sink.error(
                code::DUP_SCOPE,
                d.name.span,
                format!("duplicate scope `{}`", d.name.node),
            );
            continue;
        }
        let terms = d
            .terms
            .iter()
            .map(|t| resolve_term(t, model_index, sink))
            .collect();
        index.insert(d.name.node.clone(), scopes.len());
        scopes.push(RScope {
            name: d.name.node.clone(),
            span: d.name.span,
            terms,
        });
    }
    (scopes, index)
}

/// Resolve one `col: Type = $ctx.field` term. The RHS must be `$ctx.<field>` (one
/// segment) — anything else is `E0180` (the predicate-form rule, now at the decl).
fn resolve_term(
    t: &ScopeTerm,
    model_index: &HashMap<String, usize>,
    sink: &mut Sink,
) -> RScopeTerm {
    // The type declared here (`col: Type`) is the scope field's type .
    let ty = match &t.ty.base {
        BaseType::Primitive(p) => CtxField::Scalar(*p),
        BaseType::Model(m) => {
            if !model_index.contains_key(&m.node) {
                sink.error(
                    code::UNKNOWN_MODEL,
                    m.span,
                    format!("scope term type names unknown model `{}`", m.node),
                );
            }
            CtxField::Relation(m.node.clone())
        }
    };
    // The binding must be `$ctx.<field>` (the restricted form) — else `E0180`.
    let ctx_field = if t.ctx.name.node == "ctx" && t.ctx.path.len() == 1 {
        t.ctx.path[0].node.clone()
    } else {
        sink.error_note(
            code::SCOPE_FORM,
            t.ctx.name.span,
            "a `scope` term binds a column to `$ctx.<field>` only",
            "scope is a uniform single-owner filter parameterized by request context (auth.md)",
        );
        // Best-effort: keep the first segment (or the param name) so downstream
        // resolution has something coherent; the error already fired.
        t.ctx
            .path
            .first()
            .map(|s| s.node.clone())
            .unwrap_or_else(|| t.ctx.name.node.clone())
    };
    RScopeTerm {
        column: t.col.node.clone(),
        ctx_field,
        ty,
    }
}

/// Attach each model's `@scope Name` refs: resolve the names (`E0183`), check the
/// model carries each scope's columns at a conforming type (`E0184`, per decorator),
/// and synthesize the injected predicate + the alternative-name list.
pub fn attach_models(
    model_asts: &[&Model],
    rmodels: &mut [RModel],
    scopes: &[RScope],
    scope_index: &HashMap<String, usize>,
    sink: &mut Sink,
) {
    for (mi, ast) in model_asts.iter().enumerate() {
        if ast.scopes.is_empty() {
            continue;
        }
        let mut alts: Vec<Vec<String>> = Vec::new();
        // Terms of the first alternative feed the synthesized injection predicate.
        let mut inject_terms: Vec<&RScopeTerm> = Vec::new();
        for (ai, sref) in ast.scopes.iter().enumerate() {
            let mut names = Vec::new();
            for name in &sref.names {
                let Some(&si) = scope_index.get(&name.node) else {
                    sink.error(
                        code::SCOPE_UNKNOWN,
                        name.span,
                        format!("`@scope` names unknown scope `{}`", name.node),
                    );
                    continue;
                };
                names.push(name.node.clone());
                let scope = &scopes[si];
                for term in &scope.terms {
                    check_column(&rmodels[mi], scope, term, name.span, sink);
                    if ai == 0 {
                        inject_terms.push(term);
                    }
                }
            }
            alts.push(names);
        }
        rmodels[mi].scope = synthesize_pred(&inject_terms, ast.name.span);
        rmodels[mi].scope_alts = alts;
    }
}

/// A governed model must carry the scope's column at a conforming type (`E0184`):
/// a relation-typed term needs a forward relation to the same model; a scalar term
/// needs a scalar of the same family. The field name must equal the scope column
/// name.
fn check_column(model: &RModel, scope: &RScope, term: &RScopeTerm, at: Span, sink: &mut Sink) {
    let Some(member) = model.member(&term.column) else {
        sink.error(
            code::SCOPE_MODEL_COLUMN,
            at,
            format!(
                "`@scope {}` requires column `{}` on `{}` (the scope's column), but it has none",
                scope.name, term.column, model.name
            ),
        );
        return;
    };
    let ok = match (&term.ty, &member.kind) {
        (CtxField::Relation(target), MemberKind::Forward { target: t, .. }) => t == target,
        (CtxField::Scalar(p), MemberKind::Scalar { ty, .. }) => prim_family(*p) == prim_family(*ty),
        _ => false,
    };
    if !ok {
        sink.error_note(
            code::SCOPE_MODEL_COLUMN,
            at,
            format!(
                "column `{}` on `{}` does not conform to scope `{}`'s declared type",
                term.column, model.name, scope.name
            ),
            "the governed model's field must match the `scope` decl's `col: Type`",
        );
    }
}

/// Synthesize the injected `Predicate` from the alternative's terms — the AND of
/// each `col = $ctx.field`. Codegen lowers this exactly as the old inline `@scope`
/// predicate, so scope injection (root `WHERE`, joined `ON`, create auto-set, shard
/// key) is unchanged in effect . `None` when the alternative is empty.
fn synthesize_pred(terms: &[&RScopeTerm], span: Span) -> Option<Predicate> {
    let mut acc: Option<Predicate> = None;
    for t in terms {
        let cmp = Predicate::Cmp {
            path: Path {
                segments: vec![ident(&t.column, span)],
            },
            op: Op::Eq,
            value: Value::Param(ParamRef {
                name: ident("ctx", span),
                path: vec![ident(&t.ctx_field, span)],
            }),
        };
        acc = Some(match acc {
            None => cmp,
            Some(prev) => Predicate::And(Box::new(prev), Box::new(cmp)),
        });
    }
    acc
}

fn ident(s: &str, span: Span) -> Ident {
    Spanned {
        node: s.to_string(),
        span,
    }
}

// ---------- callable acknowledgement (E0182/E0183/E0185) ------------------

use crate::resolve::Cx;

/// Validate a callable's scope acknowledgement against the scoped models it touches
/// `touched` is the set of scoped model indices reached
/// (root + joined reaches). Enforces:
///  - `E0182`: a scoped target with neither `scoped …` nor `unscoped(…)`.
///  - `E0183`: `scoped …` names a scope decl that doesn't exist.
///  - `E0185`: the named set doesn't ⊇ ≥1 alternative of some touched model, or names
///    an axis no touched model declares.
///
/// `unscoped` opts out entirely (staleness is `W0106`, checked elsewhere).
pub fn check_ack(
    scoped: Option<&Scoped>,
    unscoped_present: bool,
    touched: &[usize],
    cx: &Cx,
    call_span: Span,
    sink: &mut Sink,
) {
    // The union of every scope name any touched model declares (its axes).
    let mut declared: Vec<&str> = Vec::new();
    for &mi in touched {
        for alt in &cx.model(mi).scope_alts {
            for n in alt {
                if !declared.contains(&n.as_str()) {
                    declared.push(n.as_str());
                }
            }
        }
    }

    let Some(s) = scoped else {
        // No `scoped …`. If the target is scoped and not opted out → E0182.
        if !touched.is_empty() && !unscoped_present {
            sink.error_note(
                code::SCOPE_MISSING_ACK,
                call_span,
                "callable touches a scoped model but declares neither `scoped …` nor `unscoped(…)`",
                "name the scope (`scoped Name`) or opt out with `unscoped(\"reason\")` — the contract is written, not implied",
            );
        }
        return;
    };

    // `scoped …` present. Every name must be a real scope decl (E0183) and declared
    // by some touched model (E0185 — an axis no touched model has).
    let named: Vec<&str> = s.names.iter().map(|n| n.node.as_str()).collect();
    for n in &s.names {
        if cx.scope(&n.node).is_none() {
            sink.error(
                code::SCOPE_UNKNOWN,
                n.span,
                format!("`scoped` names unknown scope `{}`", n.node),
            );
        } else if !declared.contains(&n.node.as_str()) {
            sink.error_note(
                code::SCOPE_ACK_MISMATCH,
                n.span,
                format!(
                    "`scoped {}` names a scope no model this callable touches declares",
                    n.node
                ),
                "drop it, or `@scope` the touched model with it",
            );
        }
    }

    // The superset rule: the named set must ⊇ ≥1 declared alternative of each touched
    // scoped model. For a single-alternative model this degenerates to
    // "names that one scope".
    for &mi in touched {
        let alts = &cx.model(mi).scope_alts;
        let satisfied = alts
            .iter()
            .any(|alt| alt.iter().all(|axis| named.contains(&axis.as_str())));
        if !alts.is_empty() && !satisfied {
            sink.error_note(
                code::SCOPE_ACK_MISMATCH,
                s.span,
                format!(
                    "`scoped …` doesn't satisfy any `@scope` alternative of `{}`",
                    cx.model(mi).name
                ),
                "name every axis of at least one of the model's `@scope` alternatives",
            );
        }
    }
}

// ---------- per-callable chosen-alternative injection  ----------------

/// Resolve the scope a callable injects **per touched scoped model**:
/// the alternative its `scoped …` clause satisfied, expanded to that alternative's
/// `(column, ctx_field)` terms. For model `M`, the chosen axes are the callable's named
/// axes that `M` declares a `@scope` for (a superset of ≥1 of `M`'s alternatives, which
/// `check_ack`/`E0185` guarantees), so `M` is always fully confined and naming extra
/// axes only narrows (never leaks). `unscoped` → empty (no injection). For a single-
/// alternative model this is that model's whole scope — byte-identical to iteration 1.
pub fn resolve_inject(
    scoped: Option<&Scoped>,
    unscoped_present: bool,
    touched: &[usize],
    cx: &Cx,
) -> Vec<ScopeInject> {
    if unscoped_present {
        return Vec::new();
    }
    let named: Vec<&str> = scoped
        .map(|s| s.names.iter().map(|n| n.node.as_str()).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for &mi in touched {
        let model = cx.model(mi);
        let mut terms: Vec<(String, String)> = Vec::new();
        let mut seen_axis: Vec<&str> = Vec::new();
        // Walk the model's alternatives in decl order; include each named axis it
        // carries, expanding the `scope` decl's terms (deduped by column).
        for alt in &model.scope_alts {
            for axis in alt {
                if !named.contains(&axis.as_str()) || seen_axis.contains(&axis.as_str()) {
                    continue;
                }
                seen_axis.push(axis.as_str());
                if let Some(scope) = cx.scope(axis) {
                    for t in &scope.terms {
                        let pair = (t.column.clone(), t.ctx_field.clone());
                        if !terms.contains(&pair) {
                            terms.push(pair);
                        }
                    }
                }
            }
        }
        if !terms.is_empty() {
            out.push(ScopeInject {
                model: model.name.clone(),
                terms,
            });
        }
    }
    out
}

/// `E0186` — every `create` on a scoped model must set a full `@scope` alternative:
/// the mutation's `scoped …` set must be a superset of ≥1 of the created model's
/// alternatives, so the engine can auto-set all of that alternative's columns from
/// `$ctx` and no row is ever created unowned. Fires at the create when the
/// named set satisfies no alternative — e.g. an `@scope A, B` (AND) model whose create
/// names only `A`, leaving `B`'s column with no `$ctx` value. Skipped for `unscoped`
/// (the auto-set is dropped and the caller owns the columns, E0181 no longer applies).
pub fn check_create_sat(m: &Mutation, cx: &Cx, sink: &mut Sink) {
    if m.unscoped.is_some() {
        return;
    }
    let named: Vec<&str> = m
        .scoped
        .as_ref()
        .map(|s| s.names.iter().map(|n| n.node.as_str()).collect())
        .unwrap_or_default();
    for stmt in &m.body {
        check_create_sat_stmt(stmt, &named, cx, sink);
    }
}

fn check_create_sat_stmt(stmt: &WriteStmt, named: &[&str], cx: &Cx, sink: &mut Sink) {
    match stmt {
        WriteStmt::Create { model, .. } => {
            let Some(mi) = cx.find(&model.node) else {
                return;
            };
            let alts = &cx.model(mi).scope_alts;
            if alts.is_empty() {
                return;
            }
            let satisfiable = alts
                .iter()
                .any(|alt| alt.iter().all(|axis| named.contains(&axis.as_str())));
            if !satisfiable {
                sink.error_note(
                    code::SCOPE_CREATE_UNSAT,
                    model.span,
                    format!(
                        "`create {}` can satisfy no `@scope` alternative — a scope column would be left unset",
                        model.node
                    ),
                    "name every axis of one `@scope` alternative in the mutation's `scoped …` so the engine auto-sets it from `$ctx`",
                );
            }
        }
        WriteStmt::Tx(inner) => {
            for s in inner {
                check_create_sat_stmt(s, named, cx, sink);
            }
        }
        _ => {}
    }
}

/// Scoped model indices a query touches: the root (if scoped) plus every scoped model
/// reached through a relation . Mirrors codegen's join sources — the
/// same reaches `ctx::collect_joined_scope` walks (a `Nest` sub-object produces no join).
pub fn touched_query(q: &Query, ti: usize, cx: &Cx) -> Vec<usize> {
    let mut out = Vec::new();
    if is_scoped(cx, ti) {
        push(&mut out, ti);
    }
    let clauses = query_clauses(q);
    let mut has_order = false;
    for c in clauses {
        match c {
            Clause::Where(p) => walk_pred_join(p, ti, cx, &mut out),
            Clause::Order(terms) => {
                has_order = true;
                for t in terms {
                    walk_path_join(&t.path, ti, cx, &mut out);
                }
            }
            _ => {}
        }
    }
    if !has_order {
        for t in &cx.model(ti).sort {
            walk_path_join(&t.path, ti, cx, &mut out);
        }
    }
    if let Some(body) = cx.shape_bodies.get(&q.ret.ty.node) {
        walk_shape_join(body, ti, cx, &mut out);
    }
    out
}

/// Scoped model indices a mutation touches: each written model (if scoped) plus the
/// scoped models its write `where`s and its declared-shape re-select join .
pub fn touched_mutation(
    m: &Mutation,
    ret_shape: Option<&str>,
    ret_model: &str,
    cx: &Cx,
) -> Vec<usize> {
    let mut out = Vec::new();
    for stmt in &m.body {
        walk_write_join(stmt, cx, &mut out);
    }
    if let (Some(name), Some(mi)) = (ret_shape, cx.find(ret_model)) {
        if let Some(body) = cx.shape_bodies.get(name) {
            walk_shape_join(body, mi, cx, &mut out);
        }
    }
    out
}

fn walk_write_join(stmt: &WriteStmt, cx: &Cx, out: &mut Vec<usize>) {
    match stmt {
        WriteStmt::Create { model, .. } => {
            if let Some(mi) = cx.find(&model.node) {
                if is_scoped(cx, mi) {
                    push(out, mi);
                }
            }
        }
        WriteStmt::Update { model, where_, .. } => {
            if let Some(mi) = cx.find(&model.node) {
                if is_scoped(cx, mi) {
                    push(out, mi);
                }
                walk_pred_join(where_, mi, cx, out);
            }
        }
        WriteStmt::Delete { model, where_ }
        | WriteStmt::HardDelete { model, where_ }
        | WriteStmt::Restore { model, where_ } => {
            if let Some(mi) = cx.find(&model.node) {
                if is_scoped(cx, mi) {
                    push(out, mi);
                }
                walk_pred_join(where_, mi, cx, out);
            }
        }
        WriteStmt::Tx(inner) => {
            for s in inner {
                walk_write_join(s, cx, out);
            }
        }
        WriteStmt::Raw(_) => {}
    }
}

fn walk_pred_join(pred: &Predicate, model: usize, cx: &Cx, out: &mut Vec<usize>) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            walk_pred_join(a, model, cx, out);
            walk_pred_join(b, model, cx, out);
        }
        Predicate::Not(p) => walk_pred_join(p, model, cx, out),
        Predicate::Cmp { path, value, .. } => {
            walk_path_join(path, model, cx, out);
            if let Value::Path(p) = value {
                walk_path_join(p, model, cx, out);
            }
        }
        Predicate::Bare(path) => walk_path_join(path, model, cx, out),
        // Filter bodies join at the call site too, but codegen does not walk them for
        // scope , so we mirror codegen and don't walk them here.
        Predicate::FilterCall { .. } | Predicate::Raw(_) => {}
    }
}

fn walk_shape_join(body: &[ShapeField], model: usize, cx: &Cx, out: &mut Vec<usize>) {
    for f in body {
        if let ShapeField::Rename {
            value: ShapeValue::Path(p),
            ..
        } = f
        {
            walk_path_join(p, model, cx, out);
        }
    }
}

/// Walk a dotted path, recording each *intermediate* scoped model entered through a
/// relation hop (a join in codegen). The terminal segment is a column, not a join.
fn walk_path_join(path: &Path, start: usize, cx: &Cx, out: &mut Vec<usize>) {
    let mut cur = start;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let Some(mem) = cx.model(cur).member(&seg.node) else {
            return;
        };
        if i + 1 == n {
            return;
        }
        match &mem.kind {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                let Some(mi) = cx.find(target) else { return };
                if is_scoped(cx, mi) {
                    push(out, mi);
                }
                cur = mi;
            }
            MemberKind::Scalar { .. } => return,
        }
    }
}

fn query_clauses(q: &Query) -> &[Clause] {
    match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare => &[],
    }
}

fn is_scoped(cx: &Cx, mi: usize) -> bool {
    !cx.model(mi).scope_alts.is_empty()
}

fn push(out: &mut Vec<usize>, mi: usize) {
    if !out.contains(&mi) {
        out.push(mi);
    }
}

/// Coarse primitive families, mirroring the operand checker (resolve.rs) so scope
/// column conformance uses the same loose bucketing (`Uuid`↔`Id`, etc.).
fn prim_family(ty: Primitive) -> u8 {
    match ty {
        Primitive::Text
        | Primitive::Uuid
        | Primitive::Id
        | Primitive::Timestamp
        | Primitive::Date => 0,
        Primitive::Int => 1,
        Primitive::Bool => 2,
        Primitive::Json => 3,
    }
}
