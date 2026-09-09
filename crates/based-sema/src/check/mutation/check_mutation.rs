use super::*;

pub(crate) fn check_mutation(m: &Mutation, cx: &Cx, sink: &mut Sink) -> Option<RMutation> {
    // A mutation returns its written row once — a stream is a read envelope.
    if m.ret.stream {
        sink.error_note(
            code::STREAM_MUTATION,
            m.ret.ty.span,
            format!("mutation `{}` cannot return a stream", m.name.node),
            "a write returns its row once; declare a stream query for the read",
        );
    }
    forbid_optional_ctx_writes(&m.body, sink);
    let params = check_mutation_params(m, sink);
    let ret = resolve_mutation_return(m, cx, sink)?;

    // At the top level there is no enclosing `tx`, so no step binding is in scope.
    // A mutation may opt out of `@scope` on its write models  — that both drops
    // the injected guard and lets a `create` assign the (otherwise engine-managed)
    // scope column, so the flag rides into every write check.
    let unscoped = m.unscoped.is_some();
    let bindings = Bindings::default();
    for stmt in &m.body {
        check_write(
            stmt,
            cx,
            &params,
            &bindings,
            m.scoped.as_ref(),
            unscoped,
            sink,
        );
    }

    // Shape-as-input eligibility for every `create … from $param` — checked at the
    // use site, with the mutation's param types in hand.
    check_from_creates(m, cx, sink);

    check_keyless_readback(m, &ret, cx, sink);

    let (shard_key, scope_inject, ctx_requires) = resolve_scope_facts(m, &ret, unscoped, cx, sink);
    Some(RMutation {
        name: m.name.node.clone(),
        span: m.span,
        guard: m.guard.as_ref().map(|g| g.node.clone()),
        ctx_requires,
        ret_model: ret.model,
        ack: m.ret.ack,
        ret_shape: ret.shape,
        shard_key,
        scope_inject,
    })
}

/// The mutation's param names, checking each default and rejecting a `?` optional filter
/// (a query-only concept — a mutation param drives a write).
fn check_mutation_params(m: &Mutation, sink: &mut Sink) -> Vec<String> {
    for p in &m.params {
        if let Some(d) = &p.default {
            resolve::check_default(d, sink);
        }
        if p.optional {
            sink.error_note(
                code::OPT_PARAM_UNFILTERED,
                p.name.span,
                format!("`?` on mutation param `{}` is not supported", p.name.node),
                "the optional filter (`name?`) is a query-only concept; a mutation param drives a write",
            );
        }
    }
    m.params.iter().map(|p| p.name.node.clone()).collect()
}

/// Resolve a mutation's return: an `-> ok` acknowledgement, else a declared shape/model —
/// which must survive a real DELETE and be a per-row shape (a write reads back one written
/// row).
fn resolve_mutation_return(m: &Mutation, cx: &Cx, sink: &mut Sink) -> Option<Resolved> {
    if m.ret.ack {
        return resolve_ack_return(m, cx, sink);
    }
    let ret = resolve_return(&m.ret, None, cx, sink)?;
    check_shape_on_real_delete(m, &ret, cx, sink);
    if ret
        .shape
        .as_deref()
        .and_then(|n| cx.shape_bodies.get(n).copied())
        .is_some_and(is_agg_shape)
    {
        sink.error_note(
            code::AGG_COMPOSE,
            m.ret.ty.span,
            format!("mutation `{}` returns an aggregate shape", m.name.node),
            "a write reads back one written row — return a per-row shape",
        );
    }
    Some(ret)
}

/// Scope acknowledgement + the routing facts threaded to codegen: verify the mutation names
/// (or opts out of) every scoped model it touches, that each scoped `create` names a full
/// alternative, warn on a stale `unscoped`, then the shard key, per-model injection, and
/// `$ctx` requirement.
fn resolve_scope_facts(
    m: &Mutation,
    ret: &Resolved,
    unscoped: bool,
    cx: &Cx,
    sink: &mut Sink,
) -> (Option<String>, Vec<ScopeInject>, Vec<CtxReq>) {
    let touched = crate::scope::touched_mutation(m, ret.shape.as_deref(), &ret.model, cx);
    crate::scope::check_ack(m.scoped.as_ref(), unscoped, &touched, cx, m.span, sink);
    // Each `create` on a scoped model must name a full `@scope` alternative so the engine
    // can auto-set its columns from `$ctx`. Skipped for `unscoped`.
    crate::scope::check_create_sat(m, cx, sink);
    // `unscoped` on a mutation touching no scope is stale.
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
    // The `$ctx` requirement derives from the same chosen alternative(s) as the
    // injection, so the bag always carries exactly the injected `:ctx_<field>`s.
    let scope_reqs = crate::scope::inject_ctx_reqs(m.scoped.as_ref(), unscoped, &touched, cx);
    let ctx_requires = crate::ctx::collect_mutation(m, cx, scope_reqs);
    (shard_key, scope_inject, ctx_requires)
}
