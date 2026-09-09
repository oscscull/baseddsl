use super::*;

/// A declared shape needs a surviving row. When every write on the return model is a real
/// DELETE, the re-select has nothing to read, so the response cannot decode as the shape.
pub(super) fn check_shape_on_real_delete(m: &Mutation, ret: &Resolved, cx: &Cx, sink: &mut Sink) {
    let mut deletes_ret = false;
    let mut survives_ret = false;
    for e in write_effects(&m.body, cx) {
        match e {
            WriteEffect::RealDelete(model) | WriteEffect::Wipe(model)
                if model.node == ret.model =>
            {
                deletes_ret = true;
            }
            WriteEffect::Surviving(model) if model.node == ret.model => survives_ret = true,
            _ => {}
        }
    }
    if deletes_ret && !survives_ret {
        sink.error_note(
            code::SHAPE_ON_DELETE,
            m.ret.ty.span,
            format!(
                "mutation `{}` performs a real DELETE of `{}` — no row survives to read back as `{}`",
                m.name.node, ret.model, m.ret.ty.node
            ),
            "a destructive mutation acknowledges instead of reading back: declare `-> ok`",
        );
    }
}
