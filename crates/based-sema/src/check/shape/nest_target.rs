use super::*;

/// The model a nested field points at: its member must exist and be a relation.
/// Reports the missing-field / not-a-relation case and returns `None`.
pub(super) fn nest_target<'a>(field: &Ident, mi: usize, cx: &'a Cx, sink: &mut Sink) -> Option<&'a str> {
    match cx.model(mi).member(&field.node).map(|m| &m.kind) {
        Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
            Some(target)
        }
        Some(MemberKind::Scalar { .. }) => {
            sink.error(
                code::SHAPE_NEST_SCALAR,
                field.span,
                format!(
                    "`{}` is a column, not a relation, so it can't be nested",
                    field.node
                ),
            );
            None
        }
        None => {
            unknown_field(cx, mi, field, sink);
            None
        }
    }
}
