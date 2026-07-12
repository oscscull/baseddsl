//! Shape / query / mutation / filter checks, including the four query inferences:
//! verb, param type, filter, and target model.

use based_ast::*;

use crate::ir::*;
use crate::resolve::{self, Cx};

// ---------- shapes ---------------------------------------------------------

pub fn check_shape(s: &Shape, cx: &Cx, sink: &mut Sink) -> Option<RShape> {
    let Some(mi) = cx.find(&s.from.node) else {
        sink.error(
            code::UNKNOWN_MODEL,
            s.from.span,
            format!(
                "shape `{}` is from unknown model `{}`",
                s.name.node, s.from.node
            ),
        );
        return None;
    };
    let mut stack = vec![s.name.node.clone()];
    check_shape_body(&s.body, mi, cx, &mut stack, sink);
    Some(RShape {
        name: s.name.node.clone(),
        from: s.from.node.clone(),
        span: s.span,
    })
}

/// `stack` is the chain of named shapes currently being expanded (the declaring
/// shape at the bottom), so a `field -> Shape` reference that closes back onto it
/// is an `E0134` error instead of infinite recursion.
fn check_shape_body(
    fields: &[ShapeField],
    mi: usize,
    cx: &Cx,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) {
    for f in fields {
        match f {
            ShapeField::Bare(id) => match cx.model(mi).member(&id.node).map(|m| &m.kind) {
                Some(MemberKind::Scalar { .. }) => {}
                Some(_) => sink.error(
                    code::SHAPE_BARE_RELATION,
                    id.span,
                    format!(
                        "relation `{}` can't be projected bare; nest it (`{} {{ … }}`) or reach a column with `=`",
                        id.node, id.node
                    ),
                ),
                None => unknown_field(cx, mi, id, sink),
            },
            // A rename reaches a column via a path, or computes one with raw SQL
            // (a leaf trapdoor — shapes have no params, so raw is left unchecked).
            ShapeField::Rename { value, .. } => {
                if let ShapeValue::Path(p) = value {
                    resolve::resolve_path(p, mi, cx, sink);
                }
            }
            ShapeField::Nest { field, body } => {
                match cx.model(mi).member(&field.node).map(|m| &m.kind) {
                    Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
                        if let Some(ti) = cx.find(target) {
                            check_shape_body(body, ti, cx, stack, sink);
                        }
                    }
                    Some(MemberKind::Scalar { .. }) => sink.error(
                        code::SHAPE_NEST_SCALAR,
                        field.span,
                        format!("`{}` is a column, not a relation, so it can't be nested", field.node),
                    ),
                    None => unknown_field(cx, mi, field, sink),
                }
            }
            // `field -> Shape`: nest a relation, projected by a named shape. The
            // reference is a pure body expansion, so the referenced shape's own decl
            // check covers its fields; here we resolve the relation, require the
            // shape's model to equal the relation target, and guard against cycles.
            ShapeField::NestRef { field, shape } => {
                match cx.model(mi).member(&field.node).map(|m| &m.kind) {
                    Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
                        match cx.shapes.get(&shape.node) {
                            Some(from) if from != target => sink.error(
                                code::SHAPE_REF_MODEL,
                                shape.span,
                                format!(
                                    "shape `{}` projects `{from}`, but `{}` relates to `{target}`",
                                    shape.node, field.node
                                ),
                            ),
                            Some(_) => check_ref_cycle(&shape.node, shape.span, cx, stack, sink),
                            None => sink.error(
                                code::SHAPE_REF_UNKNOWN,
                                shape.span,
                                format!("`-> {}` names no declared shape", shape.node),
                            ),
                        }
                    }
                    Some(MemberKind::Scalar { .. }) => sink.error(
                        code::SHAPE_NEST_SCALAR,
                        field.span,
                        format!("`{}` is a column, not a relation, so it can't be nested", field.node),
                    ),
                    None => unknown_field(cx, mi, field, sink),
                }
            }
        }
    }
}

/// Follow a `-> Shape` reference for cycle detection only: a shape that transitively
/// nests itself by reference would expand forever, so it is an error (`E0134`),
/// reported at the reference that closes the cycle.
fn check_ref_cycle(shape: &str, at: Span, cx: &Cx, stack: &mut Vec<String>, sink: &mut Sink) {
    if stack.iter().any(|s| s == shape) {
        sink.error(
            code::SHAPE_REF_CYCLE,
            at,
            format!(
                "shape reference cycle: `{}` -> `{shape}`",
                stack.join("` -> `")
            ),
        );
        return;
    }
    stack.push(shape.to_string());
    if let Some(body) = cx.shape_bodies.get(shape) {
        walk_body_refs(body, cx, stack, sink);
    }
    stack.pop();
}

/// Walk a shape body's nest structure, following each `-> Shape` reference (the
/// referenced fields themselves are checked at their own decl).
fn walk_body_refs(fields: &[ShapeField], cx: &Cx, stack: &mut Vec<String>, sink: &mut Sink) {
    for f in fields {
        match f {
            ShapeField::Nest { body, .. } => walk_body_refs(body, cx, stack, sink),
            ShapeField::NestRef { shape, .. } => {
                check_ref_cycle(&shape.node, shape.span, cx, stack, sink)
            }
            ShapeField::Bare(_) | ShapeField::Rename { .. } => {}
        }
    }
}

// ---------- queries --------------------------------------------------------

struct Resolved {
    model: String,
    shape: Option<String>,
}

pub fn check_query(q: &Query, cx: &Cx, sink: &mut Sink) -> Option<RQuery> {
    let params: Vec<String> = q.params.iter().map(|p| p.name.node.clone()).collect();
    let body_model = match &q.body {
        QueryBody::Block(s) => Some(s.model.node.as_str()),
        _ => None,
    };
    let ret = resolve_return(&q.ret, body_model, cx, sink)?;
    let ti = cx.find(&ret.model)?;

    // verb: explicit in a block, else inferred from cardinality.
    let verb = match &q.body {
        QueryBody::Block(s) => {
            if s.model.node != ret.model {
                sink.error(
                    code::RETURN_MODEL_MISMATCH,
                    s.model.span,
                    format!(
                        "statement reads `{}` but the return type is from `{}`",
                        s.model.node, ret.model
                    ),
                );
            }
            s.verb
        }
        _ => {
            if q.ret.many || q.ret.stream {
                Verb::List
            } else {
                Verb::Get
            }
        }
    };

    // A stream is a list delivered incrementally: a `get` body is a cardinality
    // mismatch (E0200).
    if q.ret.stream && verb == Verb::Get {
        sink.error_note(
            code::STREAM_GET,
            q.ret.ty.span,
            format!(
                "stream query `{}` uses `get` — a stream is many rows",
                q.name.node
            ),
            "use `list`, or drop `stream` for a scalar return",
        );
    }

    // Bare/inline queries map each param onto a same-named column (the filter);
    // block queries reference params via `$`, so no same-name mapping is required.
    let infer = !matches!(q.body, QueryBody::Block(_));
    for p in &q.params {
        check_param(p, ti, infer, cx, sink);
    }

    let mut has_order = false;
    match &q.body {
        QueryBody::Bare => {}
        QueryBody::Inline(clauses) => has_order = check_clauses(clauses, ti, cx, &params, sink),
        QueryBody::Block(s) => {
            let smi = cx.find(&s.model.node).unwrap_or(ti);
            has_order = check_clauses(&s.clauses, smi, cx, &params, sink);
        }
    }

    if verb == Verb::Get && !q.ret.stream && !get_is_keyed(q, ti, cx) {
        sink.error_note(
            code::GET_NOT_UNIQUE,
            q.span,
            format!(
                "`get` query `{}` is not keyed on a unique field",
                q.name.node
            ),
            "a scalar `get` needs an equality on `id`, a `(unique)` column, or a unique index",
        );
    }

    // Nondeterministic-order lint: a `list` with no sort at any tier.
    let paginated = matches!(&q.body, QueryBody::Inline(cs) | QueryBody::Block(Statement{clauses: cs, ..}) if cs.iter().any(|c| matches!(c, Clause::Page(_))));

    // A page is a bounded chunk + a re-entry cursor; a stream is one unbounded
    // forward pass — the envelopes contradict (E0201).
    if q.ret.stream && paginated {
        sink.error_note(
            code::STREAM_PAGE,
            q.span,
            format!("stream query `{}` declares `page`", q.name.node),
            "paginate for random access, stream for the full pass — drop one",
        );
    }
    if verb == Verb::List && !has_order && cx.model(ti).sort.is_empty() {
        sink.warn(
            code::NONDET_SORT,
            q.span,
            format!(
                "`list` query `{}` has no sort — results are nondeterministic; add `order (…)` or a model `@sort`",
                q.name.node
            ),
        );
    }

    // Scope acknowledgement: a callable touching a scoped
    // model must name it (`scoped …`) or opt out (`unscoped(…)`) — E0182/E0183/E0185.
    let touched = crate::scope::touched_query(q, ti, cx);
    crate::scope::check_ack(
        q.scoped.as_ref(),
        q.unscoped.is_some(),
        &touched,
        cx,
        q.span,
        sink,
    );

    // `unscoped` on a query with no `@scope` to opt out of is stale (W0106) — the twin
    // of W0105 for `unindexed`. Points the author at a no-op token to drop.
    if let Some(u) = &q.unscoped {
        if touched.is_empty() {
            sink.warn_note(
                code::STALE_UNSCOPED,
                u.span,
                format!(
                    "`unscoped` on query `{}` has no scope to opt out of",
                    q.name.node
                ),
                "drop it, or add `@scope Name` to a touched model",
            );
        }
    }

    let ctx_requires = crate::ctx::collect_query(q, ti, cx);

    // The shard key is the target model's `@scope` owner field  — the field the
    // request routes on — but a `unscoped` query  deliberately reads across scopes,
    // so it has no single owning shard and must route by an explicit key instead.
    let shard_key = if q.unscoped.is_some() {
        None
    } else {
        cx.model(ti).shard_key_ctx_field()
    };

    // The alternative this query injects per touched scoped model  — threaded to
    // codegen so a callable naming one `@scope` alternative filters differently from one
    // naming another. Single-alternative models resolve to the same terms as before.
    let scope_inject =
        crate::scope::resolve_inject(q.scoped.as_ref(), q.unscoped.is_some(), &touched, cx);

    Some(RQuery {
        name: q.name.node.clone(),
        span: q.span,
        target: ret.model,
        verb,
        many: q.ret.many || q.ret.stream,
        stream: q.ret.stream,
        ret_shape: ret.shape,
        paginated,
        ctx_requires,
        shard_key,
        scope_inject,
    })
}

/// Resolve a return type to its underlying model. A shape resolves via its `from`;
/// a bare model resolves to itself; `full` needs a block body naming the model.
fn resolve_return(
    ret: &RetType,
    body_model: Option<&str>,
    cx: &Cx,
    sink: &mut Sink,
) -> Option<Resolved> {
    let name = ret.ty.node.as_str();
    if name == "full" {
        return match body_model {
            Some(m) if cx.find(m).is_some() => Some(Resolved {
                model: m.to_string(),
                shape: Some("full".to_string()),
            }),
            Some(m) => {
                sink.error(
                    code::UNKNOWN_MODEL,
                    ret.ty.span,
                    format!("unknown model `{m}`"),
                );
                None
            }
            None => {
                sink.error(
                    code::FULL_NEEDS_MODEL,
                    ret.ty.span,
                    "`full` return needs a block body that names the model",
                );
                None
            }
        };
    }
    if let Some(from) = cx.shapes.get(name) {
        return Some(Resolved {
            model: from.clone(),
            shape: Some(name.to_string()),
        });
    }
    if cx.find(name).is_some() {
        return Some(Resolved {
            model: name.to_string(),
            shape: None,
        });
    }
    sink.error(
        code::UNKNOWN_RETURN,
        ret.ty.span,
        format!("unknown return type `{name}` (not a declared shape or model)"),
    );
    None
}

/// Validate a param's binding + default against the target model. When `infer` is
/// set (bare/inline query), an unbound param must name a same-named column.
fn check_param(p: &Param, ti: usize, infer: bool, cx: &Cx, sink: &mut Sink) {
    let m = cx.model(ti);
    // The column/edge this param maps onto — its type is what an explicit
    // annotation must agree with . `None` when the mapping is unresolved
    // (error already reported) or the param isn't column-mapped.
    let mapped: Option<resolve::Mapped> = match &p.binding {
        Some(ParamBinding::Edge(edge)) => match m.member(&edge.node).map(|mm| &mm.kind) {
            Some(k) if k.is_relation() => Some(resolve::Mapped::Relation(k.target().unwrap())),
            Some(_) => {
                sink.error(
                    code::BINDING_EDGE,
                    edge.span,
                    format!(
                        "`{}` is a column, not a relation, so a param can't bind via it",
                        edge.node
                    ),
                );
                None
            }
            None => {
                unknown_field(cx, ti, edge, sink);
                None
            }
        },
        Some(ParamBinding::ColOp { col, .. }) => mapped_member(m, &col.node, cx, ti, col, sink),
        // Bare/inline queries map an unbound param onto a same-named column; block
        // queries reference params via `$`, so a bare param maps to nothing.
        None if infer => match m.member(&p.name.node).map(|mm| &mm.kind) {
            Some(MemberKind::Scalar { ty, .. }) => Some(resolve::Mapped::Scalar(*ty)),
            Some(k @ (MemberKind::Forward { .. } | MemberKind::Inverse { .. })) => {
                Some(resolve::Mapped::Relation(k.target().unwrap()))
            }
            None => {
                sink.error(
                    code::UNKNOWN_FIELD,
                    p.name.span,
                    format!(
                        "param `{}` maps to a same-named column, but `{}` has none",
                        p.name.node, m.name
                    ),
                );
                None
            }
        },
        None => None,
    };

    if let (Some(ann), Some(mapped)) = (&p.ty, mapped) {
        resolve::check_param_type(ann, mapped, sink);
    }
    if let Some(d) = &p.default {
        resolve::check_default(d, sink);
    }
}

/// Resolve a member by name to its `Mapped` type, reporting an unknown-field error
/// (and returning `None`) when it doesn't exist.
fn mapped_member<'a>(
    m: &'a RModel,
    name: &str,
    cx: &Cx,
    ti: usize,
    at: &Ident,
    sink: &mut Sink,
) -> Option<resolve::Mapped<'a>> {
    match m.member(name).map(|mm| &mm.kind) {
        Some(MemberKind::Scalar { ty, .. }) => Some(resolve::Mapped::Scalar(*ty)),
        Some(k @ (MemberKind::Forward { .. } | MemberKind::Inverse { .. })) => {
            Some(resolve::Mapped::Relation(k.target().unwrap()))
        }
        None => {
            unknown_field(cx, ti, at, sink);
            None
        }
    }
}

/// Validate `where`/`order`/`page` clauses; returns whether an `order` is present.
fn check_clauses(
    clauses: &[Clause],
    mi: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) -> bool {
    let mut has_order = false;
    for c in clauses {
        match c {
            Clause::Where(p) => resolve::check_predicate(p, Some(mi), cx, params, sink),
            Clause::Order(terms) => {
                has_order = true;
                for t in terms {
                    resolve::check_sort_term(t, mi, cx, sink);
                }
            }
            Clause::Page(_) => {}
            // Validated by the index pass (indexes.rs): satisfies W0103, or is
            // itself flagged stale (W0105) when the query turns out indexed.
            Clause::Unindexed(_) => {}
        }
    }
    has_order
}

/// A `get` is validly keyed if some equality-constrained column is unique.
fn get_is_keyed(q: &Query, ti: usize, cx: &Cx) -> bool {
    let m = cx.model(ti);
    match &q.body {
        QueryBody::Block(s) => {
            let smi = cx.find(&s.model.node).unwrap_or(ti);
            let sm = cx.model(smi);
            let mut cols = Vec::new();
            for c in &s.clauses {
                if let Clause::Where(p) = c {
                    collect_eq_cols(p, &mut cols);
                }
            }
            cols.iter().any(|c| sm.is_unique(c))
        }
        _ => q.params.iter().any(|p| match &p.binding {
            None => m.is_unique(&p.name.node),
            Some(ParamBinding::ColOp { op: Op::Eq, col }) => m.is_unique(&col.node),
            _ => false,
        }),
    }
}

/// Collect single-segment columns constrained by equality anywhere in a predicate.
fn collect_eq_cols(p: &Predicate, out: &mut Vec<String>) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            collect_eq_cols(a, out);
            collect_eq_cols(b, out);
        }
        Predicate::Not(inner) => collect_eq_cols(inner, out),
        Predicate::Cmp {
            path, op: Op::Eq, ..
        } if path.segments.len() == 1 => {
            out.push(path.segments[0].node.clone());
        }
        _ => {}
    }
}

// ---------- mutations ------------------------------------------------------

pub fn check_mutation(m: &Mutation, cx: &Cx, sink: &mut Sink) -> Option<RMutation> {
    // A mutation returns its written row once — never a stream (E0202).
    if m.ret.stream {
        sink.error_note(
            code::STREAM_MUTATION,
            m.ret.ty.span,
            format!("mutation `{}` cannot return a stream", m.name.node),
            "a write returns its row once; declare a stream query for the read",
        );
    }
    let params: Vec<String> = m.params.iter().map(|p| p.name.node.clone()).collect();
    for p in &m.params {
        if let Some(d) = &p.default {
            resolve::check_default(d, sink);
        }
    }
    let ret = resolve_return(&m.ret, None, cx, sink)?;
    // At the top level there is no enclosing `tx`, so no back-reference is in scope.
    // A mutation may opt out of `@scope` on its write models  — that both drops
    // the injected guard and lets a `create` assign the (otherwise engine-managed)
    // scope column, so the flag rides into every write check.
    let unscoped = m.unscoped.is_some();
    for stmt in &m.body {
        check_write(stmt, cx, &params, None, unscoped, sink);
    }

    // Scope acknowledgement: a mutation touching a scoped
    // model must name it (`scoped …`) or opt out (`unscoped(…)`) — E0182/E0183/E0185.
    let touched = crate::scope::touched_mutation(m, ret.shape.as_deref(), &ret.model, cx);
    crate::scope::check_ack(m.scoped.as_ref(), unscoped, &touched, cx, m.span, sink);
    // E0186: each `create` on a scoped model must name a full `@scope` alternative so the
    // engine can auto-set its columns from `$ctx` (no unowned row). Skipped for `unscoped`.
    crate::scope::check_create_sat(m, cx, sink);
    // `unscoped` on a mutation touching no scope is stale (W0106).
    if let Some(u) = &m.unscoped {
        if touched.is_empty() {
            sink.warn_note(
                code::STALE_UNSCOPED,
                u.span,
                format!(
                    "`unscoped` on mutation `{}` has no scope to opt out of",
                    m.name.node
                ),
                "drop it, or add `@scope Name` to a written model",
            );
        }
    }
    // Shard key : the return model's `@scope` owner field — a `tx` is a single-shard
    // unit , so the whole mutation routes on the primary written model's owner. An
    // `unscoped` mutation  disables scope and so has no owning shard.
    let shard_key = if unscoped {
        None
    } else {
        cx.find(&ret.model)
            .and_then(|mi| cx.model(mi).shard_key_ctx_field())
    };
    let scope_inject = crate::scope::resolve_inject(m.scoped.as_ref(), unscoped, &touched, cx);
    Some(RMutation {
        name: m.name.node.clone(),
        span: m.span,
        ctx_requires: crate::ctx::collect_mutation(m, ret.shape.as_deref(), &ret.model, cx),
        ret_model: ret.model,
        ret_shape: ret.shape,
        shard_key,
        scope_inject,
    })
}

/// Check one write statement. `back` is the model of the immediately preceding
/// `create` in the enclosing `tx` (`None` at the top level or before the first
/// create) — the model a `^.field` back-reference resolves against.
fn check_write(
    stmt: &WriteStmt,
    cx: &Cx,
    params: &[String],
    back: Option<usize>,
    unscoped: bool,
    sink: &mut Sink,
) {
    match stmt {
        WriteStmt::Create { model, assigns } => {
            if let Some(mi) = write_model(model, cx, sink) {
                for a in assigns {
                    check_assign(a, mi, cx, params, back, sink);
                }
                check_scope_assign(mi, assigns, unscoped, cx, sink);
                check_create_required(mi, assigns, model, cx, sink);
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(mi) = write_model(model, cx, sink) {
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
                for a in assigns {
                    check_assign(a, mi, cx, params, back, sink);
                }
            }
        }
        WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => {
            if let Some(mi) = write_model(model, cx, sink) {
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
            }
        }
        WriteStmt::Restore { model, where_ } => {
            if let Some(mi) = write_model(model, cx, sink) {
                if cx.model(mi).soft_delete.is_none() {
                    sink.error(
                        code::RESTORE_NOT_SOFT,
                        model.span,
                        format!(
                            "`restore` requires a @soft_delete model; `{}` has none",
                            model.node
                        ),
                    );
                }
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
            }
        }
        WriteStmt::Tx(inner) => {
            // `^` reads the immediately preceding `create`; track it as we descend.
            let mut prev = back;
            for s in inner {
                check_write(s, cx, params, prev, unscoped, sink);
                if let WriteStmt::Create { model, .. } = s {
                    prev = write_model(model, cx, &mut Sink::default());
                }
            }
        }
        WriteStmt::Raw(raw) => {
            for part in &raw.parts {
                if let RawPart::Param(pr) = part {
                    resolve::check_param_ref(pr, params, sink);
                }
            }
        }
    }
}

fn check_assign(
    a: &Assign,
    mi: usize,
    cx: &Cx,
    params: &[String],
    back: Option<usize>,
    sink: &mut Sink,
) {
    let Some(member) = cx.model(mi).member(&a.col.node) else {
        unknown_field(cx, mi, &a.col, sink);
        return;
    };
    // Assigning an enum column takes a bare variant (`status = paid`), not a column
    // path — check membership (E0154) instead of resolving it as a field.
    if let MemberKind::Scalar {
        enum_name: Some(en_name),
        ..
    } = &member.kind
    {
        if let Some(en) = cx.enum_(en_name) {
            if resolve::check_enum_operand(&a.value, en, params, sink) {
                return;
            }
        }
    }
    // A `^.field` back-reference resolves against the preceding create's model, not
    // the model being assigned; delegate the rest of the value to the shared checker.
    if let Value::Back(b) = &a.value {
        check_back(b, back, cx, sink);
    } else {
        resolve::check_value(&a.value, Some(mi), cx, params, sink);
    }
    // The assigned value's type must agree with the target column (E0153); silent
    // when either side failed to resolve above.
    resolve::check_assign_type(&member.kind, &a.col, &a.value, mi, back, cx, sink);
}

/// On a scoped model  the `@scope` column is engine-managed on `create`: it is
/// auto-set from `$ctx.<field>` so a caller cannot plant a row outside their own scope
/// (cross-scope create is inexpressible). Assigning it is therefore an `E0181`, exactly
/// as assigning `id`/`@created` would be redundant/wrong — *unless* the mutation is
/// `unscoped` , in which case scope isn't injected at all and the caller owns the
/// column. Reports every offending assign.
fn check_scope_assign(mi: usize, assigns: &[Assign], unscoped: bool, cx: &Cx, sink: &mut Sink) {
    if unscoped {
        return;
    }
    let m = cx.model(mi);
    let scope_cols = m.scope_terms();
    for a in assigns {
        if scope_cols.iter().any(|(f, _)| f == &a.col.node) {
            sink.error_note(
                code::SCOPE_ASSIGN,
                a.col.span,
                format!(
                    "`{}` is `@scope`-managed on `create`; the engine sets it from `$ctx`",
                    a.col.node
                ),
                "a scoped create can't target another scope — drop the assign, or mark the mutation `unscoped(\"…\")`",
            );
        }
    }
}

/// A `create` must assign every *required* column: a non-optional, non-defaulted
/// stored column or forward FK. Engine-managed fields — the `id`, `@created` /
/// `@updated` timestamps, the `@soft_delete` field, and any `@scope` column
/// (auto-set from `$ctx` on insert) — are set by the engine, so they are
/// exempt. Inverse edges own no column here, so they never count. A missing field
/// is `E0146` (all missing fields reported in one error).
fn check_create_required(mi: usize, assigns: &[Assign], at: &Ident, cx: &Cx, sink: &mut Sink) {
    let m = cx.model(mi);
    let assigned: Vec<&str> = assigns.iter().map(|a| a.col.node.as_str()).collect();
    let scope_cols = m.scope_terms();
    let managed = |name: &str| {
        name == "id"
            || m.created.as_deref() == Some(name)
            || m.updated.as_deref() == Some(name)
            || m.soft_delete.as_ref().map(|s| s.field.as_str()) == Some(name)
            || scope_cols.iter().any(|(f, _)| f == name)
    };
    let missing: Vec<&str> = m
        .members
        .iter()
        .filter(|mem| is_required(&mem.kind))
        .map(|mem| mem.name.as_str())
        .filter(|name| !managed(name) && !assigned.contains(name))
        .collect();
    if !missing.is_empty() {
        sink.error(
            code::CREATE_MISSING,
            at.span,
            format!(
                "`create {}` is missing required field{}: {}",
                m.name,
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ),
        );
    }
}

/// A column the caller must supply on `create`: a non-optional scalar with no
/// default, or a non-optional forward FK (a custom-join edge has no FK column to
/// set, so it is excluded).
fn is_required(kind: &MemberKind) -> bool {
    match kind {
        MemberKind::Scalar {
            optional, default, ..
        } => !*optional && default.is_none(),
        MemberKind::Forward {
            optional,
            custom_join,
            ..
        } => !*optional && !*custom_join,
        MemberKind::Inverse { .. } => false,
    }
}

/// Resolve a `^.field` back-reference: there must be a preceding `create` in the
/// enclosing `tx` (`back`), and `field` must be one of its columns.
fn check_back(b: &BackRef, back: Option<usize>, cx: &Cx, sink: &mut Sink) {
    match back {
        None => sink.error(
            code::BACKREF_SCOPE,
            b.span,
            "`^` needs a preceding `create` in the same `tx`",
        ),
        Some(mi) => {
            if cx.model(mi).member(&b.field.node).is_none() {
                unknown_field(cx, mi, &b.field, sink);
            }
        }
    }
}

fn write_model(name: &Ident, cx: &Cx, sink: &mut Sink) -> Option<usize> {
    match cx.find(&name.node) {
        Some(i) => Some(i),
        None => {
            sink.error(
                code::UNKNOWN_MODEL,
                name.span,
                format!("unknown model `{}`", name.node),
            );
            None
        }
    }
}

// ---------- filters --------------------------------------------------------

pub fn check_filter(f: &NamedFilter, cx: &Cx, sink: &mut Sink) -> RFilter {
    let params: Vec<String> = f.params.iter().map(|p| p.name.node.clone()).collect();
    for p in &f.params {
        if let Some(d) = &p.default {
            resolve::check_default(d, sink);
        }
    }
    // A named filter has no caller model at declaration, so column paths are not
    // bound here (they resolve against whichever model calls it) — only params,
    // nested filter calls, and functions are checked.
    resolve::check_predicate(&f.pred, None, cx, &params, sink);
    RFilter {
        name: f.name.node.clone(),
        span: f.span,
        arity: f.params.len(),
    }
}

// ---------- shared ---------------------------------------------------------

fn unknown_field(cx: &Cx, mi: usize, id: &Ident, sink: &mut Sink) {
    sink.error(
        code::UNKNOWN_FIELD,
        id.span,
        format!("`{}` has no field `{}`", cx.model(mi).name, id.node),
    );
}
