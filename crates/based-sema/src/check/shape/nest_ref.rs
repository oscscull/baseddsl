use super::*;

/// The `field -> Shape` half of a nest-by-reference: the named shape must exist, project
/// the relation's target model, and not be an aggregate; the reference must not cycle.
pub(super) fn check_nest_ref(
    shape: &Ident,
    field: &Ident,
    target: &str,
    cx: &Cx,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) {
    match cx.shapes.get(&shape.node) {
        Some(from) if from != target => sink.error(
            code::SHAPE_REF_MODEL,
            shape.span,
            format!(
                "shape `{}` projects `{from}`, but `{}` relates to `{target}`",
                shape.node, field.node
            ),
        ),
        Some(_) => {
            if cx
                .shape_bodies
                .get(&shape.node)
                .is_some_and(|b| is_agg_shape(b))
            {
                sink.error_note(
                    code::AGG_COMPOSE,
                    shape.span,
                    format!("`{}` is an aggregate shape", shape.node),
                    "an aggregate shape is a group, not a row — it can't be nested",
                );
            }
            check_ref_cycle(&shape.node, shape.span, cx, stack, sink);
        }
        None => sink.error(
            code::SHAPE_REF_UNKNOWN,
            shape.span,
            format!("`-> {}` names no declared shape", shape.node),
        ),
    }
}
