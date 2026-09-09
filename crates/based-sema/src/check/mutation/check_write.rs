use super::*;

pub(super) fn check_write(
    stmt: &WriteStmt,
    cx: &Cx,
    params: &[String],
    bindings: &Bindings,
    scoped: Option<&Scoped>,
    unscoped: bool,
    sink: &mut Sink,
) {
    match stmt {
        WriteStmt::Create {
            model,
            assigns,
            from,
            conflict,
            binding: _,
        } => {
            if let Some(mi) = write_model(model, cx, sink) {
                // The structured `create … from $param` form carries no inline assigns;
                // its shape-as-input eligibility is checked by `check_from_creates` (it
                // needs the mutation's param types, threaded there, not here).
                if from.is_some() {
                    let _ = mi;
                    return;
                }
                for a in assigns {
                    check_assign(
                        a, mi, cx, params, bindings, /* in_update = */ false, sink,
                    );
                }
                check_scope_assign(mi, assigns, unscoped, cx, sink);
                check_create_required(mi, assigns, model, scoped, unscoped, cx, sink);
                if let Some(oc) = conflict {
                    check_upsert(oc, mi, assigns, scoped, unscoped, cx, params, sink);
                }
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            if let Some(mi) = write_model(model, cx, sink) {
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
                for a in assigns {
                    check_assign(
                        a, mi, cx, params, bindings, /* in_update = */ true, sink,
                    );
                }
            }
        }
        WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => {
            if let Some(mi) = write_model(model, cx, sink) {
                // `where_` is `None` for the `delete all` whole-table wipe.
                if let Some(pred) = where_ {
                    resolve::check_predicate(pred, Some(mi), cx, params, sink);
                }
            }
        }
        WriteStmt::Restore { model, where_ } => {
            if let Some(mi) = write_model(model, cx, sink) {
                if cx.model(mi).soft_delete.is_none() {
                    sink.error(
                        code::RESTORE_NOT_SOFT,
                        model.span,
                        format!(
                            "`restore` requires a @soft_delete model; `{}` has none",
                            model.node
                        ),
                    );
                }
                resolve::check_predicate(where_, Some(mi), cx, params, sink);
            }
        }
        WriteStmt::Tx(inner) => check_tx(inner, cx, params, bindings, scoped, unscoped, sink),
        WriteStmt::Raw(raw) => {
            for part in &raw.parts {
                if let RawPart::Param(pr) = part {
                    resolve::check_param_ref(pr, params, sink);
                }
            }
        }
    }
}
