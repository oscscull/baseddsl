use super::*;

/// Validate a bulk upsert's `on conflict (target) update { … }` on a `create … from`.
/// The conflict-target rules are the singular upsert's, applied to the input shape's
/// columns: the target is a unique key whose columns the shape sets, the model is free of
/// `@soft_delete`, and a scoped model carries its scope columns. The `update` branch is an
/// ordinary assign block whose operands may reference the **stored** row (a bare column), a
/// param/literal, self-referential arithmetic, and the **incoming** proposed row
/// (`incoming.<col>`).
pub(super) fn check_bulk_upsert(
    oc: &OnConflict,
    mi: usize,
    m: &Mutation,
    cf: &CreateFrom,
    cx: &Cx,
    sink: &mut Sink,
) {
    let m_model = cx.model(mi);
    let target: Vec<&str> = oc.target.iter().map(|t| t.node.as_str()).collect();

    // The input shape's covered (settable) columns supply the conflict value — the bulk
    // counterpart of "assigned in the create block".
    let shape_cols = input_shape_columns(m, cf, cx);
    check_conflict_target_over(
        oc,
        &target,
        mi,
        &shape_cols,
        m.scoped.as_ref(),
        m.unscoped.is_some(),
        cx,
        sink,
    );

    // The update branch: reject an assign that moves the key, validate `incoming.<col>`
    // references, and reference/type-check every other operand (bare stored column / param /
    // literal / arithmetic).
    let no_bindings = Bindings::default();
    let params: Vec<String> = m.params.iter().map(|p| p.name.node.clone()).collect();
    for a in &oc.update {
        if target.iter().any(|t| *t == a.col.node) {
            sink.error_note(
                code::UPSERT_TARGET_SET,
                a.col.span,
                format!(
                    "the `on conflict update` branch assigns the conflict column `{}`",
                    a.col.node
                ),
                "the update runs on a conflict *of* this key — don't move it",
            );
        }
        // Validate the LHS is a settable column of the model.
        if m_model.member(&a.col.node).is_none() {
            unknown_field(cx, mi, &a.col, sink);
            continue;
        }
        // Validate every `incoming.<col>` operand names a settable column.
        check_incoming_refs(&a.value, m_model, cx, mi, sink);
        // Non-incoming operands get the ordinary assign check; an assign that mentions
        // `incoming` is validated by the walk above (its type agreement is the column's).
        if rhs_incoming_span(&a.value).is_none() {
            check_assign(
                a,
                mi,
                cx,
                &params,
                &no_bindings,
                /* in_update = */ true,
                sink,
            );
        }
    }

    // `...incoming` spreads the proposed row's payload columns (minus the target) — the only
    // spreadable source is `incoming`.
    if let Some(sp) = &oc.spread {
        if sp.source.node != "incoming" {
            sink.error_note(
                code::INPUT_INCOMING_CONTEXT,
                sp.source.span,
                format!(
                    "`...{}` is not a valid spread — only `...incoming` is allowed here",
                    sp.source.node
                ),
                "`...incoming` splices the proposed row's columns into the update",
            );
        }
    }
}
