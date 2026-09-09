use super::*;

pub(crate) fn unknown_field(cx: &Cx, mi: usize, id: &Ident, sink: &mut Sink) {
    sink.error(
        code::UNKNOWN_FIELD,
        id.span,
        format!("`{}` has no field `{}`", cx.model(mi).name, id.node),
    );
}
