use super::*;

/// A create on a keyless return model has no generated `id` to read the row back by, so a
/// declared-shape return must key on a `(unique)` column the create sets. A create that
/// sets none is rejected — the runtime would have no key for the re-select. An `-> ok`
/// mutation reads nothing back, so it is exempt.
pub(super) fn check_keyless_readback(m: &Mutation, ret: &Resolved, cx: &Cx, sink: &mut Sink) {
    if m.ret.ack {
        return;
    }
    let Some(rmodel) = cx.find(&ret.model).map(|i| cx.model(i)) else {
        return;
    };
    if rmodel.no_id
        && creates_model(&m.body, &ret.model)
        && !create_sets_unique(&m.body, &ret.model, rmodel)
    {
        sink.error_note(
            code::KEYLESS_CREATE,
            m.span,
            format!(
                "mutation `{}` creates keyless `{}` but sets no unique column to read it back by",
                m.name.node, ret.model
            ),
            "a `@no_id` model has no generated `id` — assign a `(unique)` column in the `create`, or return `-> ok`",
        );
    }
}

/// Whether the mutation body creates a row of `model` (recursing into `tx`).
fn creates_model(body: &[WriteStmt], model: &str) -> bool {
    body.iter().any(|w| match w {
        WriteStmt::Create { model: m, .. } => m.node == model,
        WriteStmt::Tx(inner) => creates_model(inner, model),
        _ => false,
    })
}

/// Whether every `create` of `model` in the body assigns a `(unique)` column — the
/// read-back key a keyless model needs (no generated `id`). Fires per create so a
/// mixed batch is only clean when each keyless create is keyable.
fn create_sets_unique(body: &[WriteStmt], model: &str, rmodel: &RModel) -> bool {
    body.iter().all(|w| match w {
        WriteStmt::Create {
            model: m, assigns, ..
        } if m.node == model => assigns.iter().any(|a| rmodel.is_unique(&a.col.node)),
        WriteStmt::Tx(inner) => create_sets_unique(inner, model, rmodel),
        _ => true,
    })
}
