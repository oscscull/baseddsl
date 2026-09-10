use super::*;

/// Follow a `-> Shape` reference for cycle detection only: a shape that transitively nests
/// itself by reference would expand forever, so it is an error reported at the reference
/// that closes the cycle.
pub(super) fn check_ref_cycle(
    shape: &str,
    at: Span,
    cx: &Cx,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) {
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
            ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
                walk_body_refs(body, cx, stack, sink);
            }
            ShapeField::NestRef { shape, .. } => {
                check_ref_cycle(&shape.node, shape.span, cx, stack, sink);
            }
            ShapeField::Bare(_) | ShapeField::Rename { .. } | ShapeField::Spread { .. } => {}
        }
    }
}
