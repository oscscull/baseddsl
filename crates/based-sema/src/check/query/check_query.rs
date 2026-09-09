use super::*;

pub(crate) fn check_query(q: &Query, cx: &Cx, sink: &mut Sink) -> Option<RQuery> {
    // `-> ok` acknowledges a destructive mutation; a query returns data.
    if q.ret.ack {
        sink.error_note(
            code::ACK_QUERY,
            q.ret.ty.span,
            format!("query `{}` cannot return `ok`", q.name.node),
            "a query returns data — declare a shape or model; `-> ok` is for destructive mutations",
        );
        return None;
    }
    let params: Vec<String> = q.params.iter().map(|p| p.name.node.clone()).collect();
    let body_model = match &q.body {
        QueryBody::Block(s) => Some(s.model.node.as_str()),
        _ => None,
    };
    let ret = resolve_return(&q.ret, body_model, cx, sink)?;
    let ti = cx.find(&ret.model)?;

    let verb = query_verb(q, &ret.model, sink);

    // Bare/inline queries map each param onto a same-named column (the filter);
    // block and raw queries reference params via `$`, so no same-name mapping is
    // required.
    let infer = matches!(q.body, QueryBody::Bare | QueryBody::Inline(_));
    for p in &q.params {
        check_param(p, ti, infer, cx, sink);
    }
    check_optional_params(q, verb, sink);
    check_optional_ctx_query(q, sink);

    // An aggregate return shape turns the query into an aggregate query: `group by` /
    // `having` become legal (and required for consistency), and the `get`/sort/pagination
    // rules change.
    let agg_body: Option<&[ShapeField]> = ret
        .shape
        .as_deref()
        .and_then(|n| cx.shape_bodies.get(n).copied())
        .filter(|b| is_agg_shape(b));

    let shape = QueryShape {
        verb,
        raw: matches!(q.body, QueryBody::Raw(_)),
        agg: agg_body.is_some(),
        has_order: check_query_body(q, ti, agg_body, cx, &params, sink),
        paginated: matches!(&q.body, QueryBody::Inline(cs) | QueryBody::Block(Statement{clauses: cs, ..}) if cs.iter().any(|c| matches!(c, Clause::Page(_)))),
    };
    check_query_envelope(q, ti, &shape, cx, sink);
    check_distinct(q, &ret, ti, shape.agg, cx, sink);
    check_for_update(q, &ret, ti, shape.agg, cx, sink);

    let (shard_key, scope_inject, ctx_requires) = resolve_scope_facts(q, ti, cx, sink);

    Some(RQuery {
        name: q.name.node.clone(),
        span: q.span,
        target: ret.model,
        verb,
        many: q.ret.many || q.ret.stream,
        stream: q.ret.stream,
        ret_shape: ret.shape,
        paginated: shape.paginated,
        ctx_requires,
        shard_key,
        scope_inject,
    })
}

/// Scope acknowledgement + the routing facts threaded to codegen: verify the query names
/// (or opts out of) every scoped model it touches, warn on a stale `unscoped`, then the
/// shard key, per-model injection, and `$ctx` requirement.
fn resolve_scope_facts(
    q: &Query,
    ti: usize,
    cx: &Cx,
    sink: &mut Sink,
) -> (Option<String>, Vec<ScopeInject>, Vec<CtxReq>) {
    let touched = crate::scope::touched_query(q, ti, cx);
    crate::scope::check_ack(
        q.scoped.as_ref(),
        q.unscoped.is_some(),
        &touched,
        cx,
        q.span,
        sink,
    );

    // `unscoped` on a query with no `@scope` to opt out of is stale — a redundant token to
    // drop, the twin of the stale-`unindexed` warning.
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

    // The shard key is the target model's `@scope` owner field  — the field the
    // request routes on — but a `unscoped` query  deliberately reads across scopes,
    // so it has no single owning shard and must route by an explicit key instead.
    let shard_key = if q.unscoped.is_some() {
        None
    } else {
        cx.model(ti).shard_key_ctx_field()
    };

    // The alternative this query injects per touched scoped model — threaded to codegen
    // so a callable naming one `@scope` alternative filters differently from one naming
    // another. The `$ctx` requirement derives from the same choice, so the ctx bag always
    // carries exactly the fields the injected `:ctx_<field>` binds read.
    let scope_inject =
        crate::scope::resolve_inject(q.scoped.as_ref(), q.unscoped.is_some(), &touched, cx);
    let scope_reqs =
        crate::scope::inject_ctx_reqs(q.scoped.as_ref(), q.unscoped.is_some(), &touched, cx);
    let ctx_requires = crate::ctx::collect_query(q, ti, cx, scope_reqs);
    (shard_key, scope_inject, ctx_requires)
}

/// What the body turned out to be — the facts the envelope lints (keying, sort
/// determinism, stream/page exclusivity) are judged against.
pub(super) struct QueryShape {
    pub(crate) verb: Verb,
    pub(crate) raw: bool,
    pub(crate) agg: bool,
    /// The body carries its own `order` clause.
    pub(crate) has_order: bool,
    pub(crate) paginated: bool,
}
