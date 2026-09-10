use super::*;

/// A leading-`incoming` dotted path (`incoming.<col>`) — the bulk-upsert incoming-row
/// keyword. Two segments, the first exactly `incoming`.
fn is_incoming_path(p: &Path) -> bool {
    p.segments.len() == 2 && p.segments[0].node == "incoming"
}

/// The span of the first `incoming.<col>` operand in an assign RHS (walking arithmetic),
/// or `None` when no operand is one.
pub(in crate::check::mutation) fn rhs_incoming_span(rhs: &AssignRhs) -> Option<Span> {
    match rhs {
        AssignRhs::Value(Value::Path(p)) if is_incoming_path(p) => Some(p.segments[0].span),
        AssignRhs::Value(_) => None,
        AssignRhs::Arith { lhs, rhs, .. } => {
            rhs_incoming_span(lhs).or_else(|| rhs_incoming_span(rhs))
        }
    }
}

/// Validate every `incoming.<col>` operand of an assign RHS: `<col>` must be a settable
/// scalar column of the model. Walks arithmetic operands.
pub(super) fn check_incoming_refs(
    rhs: &AssignRhs,
    model: &RModel,
    cx: &Cx,
    mi: usize,
    sink: &mut Sink,
) {
    match rhs {
        AssignRhs::Value(Value::Path(p)) if is_incoming_path(p) => {
            let field = &p.segments[1];
            let settable = model.member(&field.node).is_some_and(|mem| {
                matches!(mem.kind, MemberKind::Scalar { .. }) && mem.kind.opaque().is_none()
            });
            if !settable {
                let _ = (cx, mi);
                sink.error_note(
                    code::INPUT_INCOMING_FIELD,
                    field.span,
                    format!("`incoming.{}` is not a settable column of `{}`", field.node, model.name),
                    "`incoming.<col>` reads the proposed row's value — name a scalar column of the model",
                );
            }
        }
        AssignRhs::Value(_) => {}
        AssignRhs::Arith { lhs, rhs, .. } => {
            check_incoming_refs(lhs, model, cx, mi, sink);
            check_incoming_refs(rhs, model, cx, mi, sink);
        }
    }
}
