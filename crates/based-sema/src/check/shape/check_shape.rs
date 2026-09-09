use super::*;

pub(crate) fn check_shape(s: &Shape, cx: &Cx, sink: &mut Sink) -> Option<RShape> {
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
    // An aggregate shape projects groups: it must be flat (a group has no sub-objects, so
    // no relation nest or reference) and carry no per-row computed field.
    if is_agg_shape(&s.body) {
        for f in &s.body {
            if let ShapeField::Nest { field, .. }
            | ShapeField::NestRef { field, .. }
            | ShapeField::Flatten { out: field, .. } = f
            {
                sink.error_note(
                    code::AGG_COMPOSE,
                    field.span,
                    format!("aggregate shape `{}` nests `{}`", s.name.node, field.node),
                    "an aggregate shape is flat — project columns and aggregates, not sub-objects",
                );
            }
            if let ShapeField::Rename {
                out,
                value: ShapeValue::Computed(_),
            } = f
            {
                sink.error_note(
                    code::CFIELD_IN_AGG,
                    out.span,
                    format!(
                        "aggregate shape `{}` also computes the per-row field `{}`",
                        s.name.node, out.node
                    ),
                    "a computed field is per-row; an aggregate shape is over groups — split them into separate shapes",
                );
            }
        }
    }
    Some(RShape {
        name: s.name.node.clone(),
        from: s.from.node.clone(),
        span: s.span,
    })
}
