use super::*;

/// Resolve an `-> ok` mutation: `ok` is the **universal opt-out of read-back** — any
/// mutation may forfeit the declared-shape return. The primary model — the one scope,
/// sharding, and (for a real DELETE) the 404-on-zero-rows check ride on — is the first
/// non-raw write's model. A surviving write under `-> ok` is legal (a bulk
/// `create Model[] from $rows -> ok` is the motivating case). A body of only raw writes
/// still needs a primary model to hang scope/sharding on, so it is an error.
pub(super) fn resolve_ack_return(m: &Mutation, cx: &Cx, sink: &mut Sink) -> Option<Resolved> {
    let mut primary: Option<&Ident> = None;
    for e in write_effects(&m.body, cx) {
        match e {
            WriteEffect::RealDelete(model)
            | WriteEffect::Wipe(model)
            | WriteEffect::Surviving(model) => {
                primary = primary.or(Some(model));
            }
            WriteEffect::Raw => {}
        }
    }
    let Some(model) = primary else {
        sink.error_note(
            code::ACK_SURVIVING,
            m.ret.ty.span,
            format!(
                "mutation `{}` returns `ok` but has no engine-known write",
                m.name.node
            ),
            "`-> ok` acknowledges a write (create / update / delete / …); a raw-only body has no primary model",
        );
        return None;
    };
    // An unknown model is reported by the write check at its own site.
    cx.find(&model.node)?;
    Some(Resolved {
        model: model.node.clone(),
        shape: None,
    })
}
