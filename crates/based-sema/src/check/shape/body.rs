use super::*;

/// `stack` is the chain of named shapes currently being expanded (the declaring shape at
/// the bottom), so a `field -> Shape` reference that closes back onto it is caught as a
/// cycle error.
pub(super) fn check_shape_body(
    fields: &[ShapeField],
    mi: usize,
    cx: &Cx,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) {
    for f in fields {
        match f {
            ShapeField::Bare(id) => {
                // A composite-key relation projects bare as its structured id object (one
                // sub-column per key part); every other relation must nest or reach a column.
                let composite_fk = match cx.model(mi).member(&id.node).map(|m| &m.kind) {
                    Some(MemberKind::Forward { target, .. }) => cx
                        .find(target)
                        .is_some_and(|i| cx.model(i).is_composite_key()),
                    _ => false,
                };
                match cx.model(mi).member(&id.node).map(|m| &m.kind) {
                    Some(MemberKind::Scalar { .. }) => {}
                    Some(MemberKind::Forward { .. }) if composite_fk => {}
                    Some(_) => sink.error(
                        code::SHAPE_BARE_RELATION,
                        id.span,
                        format!(
                            "relation `{}` can't be projected bare; nest it (`{} {{ … }}`) or reach a column with `=`",
                            id.node, id.node
                        ),
                    ),
                    None => unknown_field(cx, mi, id, sink),
                }
            }
            // A rename reaches a column via a path, computes one with raw SQL (a leaf
            // trapdoor — shapes have no params, so raw is left unchecked), or aggregates
            // a column (`= count()` / `= sum(total)`).
            ShapeField::Rename { value, .. } => match value {
                ShapeValue::Path(p) => {
                    resolve::resolve_path(p, mi, cx, sink);
                }
                ShapeValue::Raw(_) => {}
                ShapeValue::Agg(agg) => check_agg_call(agg, mi, cx, sink),
                // A per-row derived scalar (`out = price - discount` / `a || b` / `case …`):
                // type-check the expression against this model.
                ShapeValue::Computed(expr) => resolve::check_shape_expr(expr, mi, cx, sink),
            },
            ShapeField::Nest { field, body } => {
                if let Some(ti) = nest_target(field, mi, cx, sink).and_then(|t| cx.find(t)) {
                    check_shape_body(body, ti, cx, stack, sink);
                }
            }
            // `field -> Shape`: nest a relation, projected by a named shape. The
            // reference is a pure body expansion, so the referenced shape's own decl
            // check covers its fields; here we resolve the relation, require the
            // shape's model to equal the relation target, and guard against cycles.
            ShapeField::NestRef { field, shape } => {
                if let Some(target) = nest_target(field, mi, cx, sink) {
                    check_nest_ref(shape, field, target, cx, stack, sink);
                }
            }
            // `out = path { body }`: flatten a to-many path through a junction to the
            // far side (the junction is hidden). The path resolves as a to-many inverse
            // hop then forward hops; the body projects the far model.
            ShapeField::Flatten { path, body, .. } => {
                if let Some(far) = check_flatten_path(path, mi, cx, sink) {
                    if cx.model(far).no_id {
                        let span = path
                            .segments
                            .last()
                            .map_or(path.segments[0].span, |s| s.span);
                        sink.error_note(
                            code::FLATTEN_KEYLESS,
                            span,
                            format!("`{}` is a keyless (`@no_id`) model", cx.model(far).name),
                            "a flattening projection returns a distinct *set* of far rows — it needs a primary key to dedup on",
                        );
                    }
                    check_shape_body(body, far, cx, stack, sink);
                }
            }
            // Composition spreads are expanded to concrete fields before sema runs
            // (based_sema::expand_spreads), so none reach here.
            ShapeField::Spread { .. } => {}
        }
    }
}
