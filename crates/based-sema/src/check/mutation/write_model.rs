use super::*;

pub(super) fn write_model(name: &Ident, cx: &Cx, sink: &mut Sink) -> Option<usize> {
    if let Some(i) = cx.find(&name.node) {
        Some(i)
    } else {
        sink.error(
            code::UNKNOWN_MODEL,
            name.span,
            format!("unknown model `{}`", name.node),
        );
        None
    }
}
