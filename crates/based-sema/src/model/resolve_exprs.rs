use super::*;

/// Read-only pass over one model's expression-valued decorators/fields: model
/// `@sort` terms, relation-field `@sort` terms, and custom `on:` joins. Run after
/// every model is built, so path traversal into other models is safe. (`@scope`
/// refs are resolved separately by the scope pass.)
pub fn resolve_exprs(ast: &Model, cx: &resolve::Cx, sink: &mut Sink) {
    let Some(mi) = cx.find(&ast.name.node) else {
        return;
    };
    for d in &ast.decorators {
        // Model `@sort` paths traverse into related models, so resolve them here in the
        // read pass. (`@scope` refs are resolved separately by the scope pass.)
        if d.name.node == "sort" {
            for a in &d.args {
                if let Some(t) = deco_sort_term(a) {
                    resolve::check_sort_term(&t, mi, cx, sink);
                }
            }
        }
    }
    // Custom `on:` joins span two tables (this model + the relation target), so
    // resolve them here in the read pass where other models are reachable .
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(pred) = &f.relation_on else { continue };
        match &f.ty.base {
            // A to-one forward relation — the only edge that owns a join. `on:` on a
            // scalar, an optional is fine; a `[]` / explicit-inverse edge owns no FK.
            BaseType::Model(target) if !f.ty.many && f.inverse.is_none() => {
                if let Some(fi) = cx.find(&target.node) {
                    // A self-ref custom join writes both sides with the same table name
                    // (`node.parent_ref = node.id`), so neither the source nor codegen can
                    // tell the near row from the joined row. Reject it — self-relations use
                    // the `<field>_id` convention, which aliases the two sides distinctly.
                    if fi == mi {
                        sink.error(
                            code::JOIN_SELF_REF,
                            f.name.span,
                            format!(
                                "`on:` custom join on `{}` is self-referential (both sides are `{}`) — \
                                 its two sides can't be named apart; use the `<field>_id` convention for a self-relation",
                                f.name.node, target.node
                            ),
                        );
                    } else {
                        resolve::check_relation_on(pred, mi, fi, cx, sink);
                    }
                }
            }
            _ => sink.error(
                code::JOIN_FORM,
                f.name.span,
                format!(
                    "`on:` custom join applies only to a to-one relation, not `{}`",
                    f.name.node
                ),
            ),
        }
    }

    // A field-level `@sort` orders the target's rows when reached via this edge — which
    // only exists for a to-many relation (its nested array). On a scalar or a to-one
    // relation there is no collection to order, so codegen drops it; reject the misplaced
    // form instead of silently ignoring it. On a valid to-many edge the terms
    // resolve against the target model.
    for mem in &ast.members {
        let Member::Field(f) = mem else { continue };
        let Some(terms) = &f.sort else { continue };
        let target = match cx.model(mi).member(&f.name.node).map(|m| &m.kind) {
            // To-many inverse edge (its paired forward FK is not unique): a genuine
            // collection. Resolve the sort terms against the target model.
            Some(MemberKind::Inverse { target, via }) => match cx.find(target) {
                Some(ti) if !cx.model(ti).is_unique(via) => Some(ti),
                // A to-one inverse (has-one) has no collection to order.
                Some(_) => None,
                // Unresolved target: a relation error was already reported — don't pile on.
                None => continue,
            },
            // A scalar or forward (to-one) relation: nothing to order.
            _ => None,
        };
        match target {
            Some(ti) => {
                for t in terms {
                    resolve::check_sort_term(t, ti, cx, sink);
                }
            }
            None => sink.error_note(
                code::SORT_MISPLACED,
                f.name.span,
                format!(
                    "`@sort` on `{0}` has no effect: a field `@sort` orders a to-many relation's nested collection, and `{0}` is a scalar or to-one relation (nothing to order)",
                    f.name.node
                ),
                "a field `@sort` is valid only on a to-many relation field; the model-wide default `@sort(…)` goes before the model, not inside its body",
            ),
        }
    }
}
