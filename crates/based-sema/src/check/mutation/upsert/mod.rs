//! Inline `on conflict` upsert: target rules + the update branch.

use super::*;

mod conflict_target;

pub(in crate::check::mutation) use conflict_target::check_conflict_target_over;
use conflict_target::*;

/// Validate an upsert's `on conflict (target) update { … }`: the target
/// must be a declared unique key each of whose columns the create sets, the `update`
/// branch is an ordinary update that may not move the key, and (safety) a soft-delete
/// model is out and a scoped model's target must carry its scope column(s).
#[allow(clippy::too_many_arguments)]
pub(super) fn check_upsert(
    oc: &OnConflict,
    mi: usize,
    create_assigns: &[Assign],
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    let target: Vec<&str> = oc.target.iter().map(|t| t.node.as_str()).collect();
    check_conflict_target(oc, &target, mi, create_assigns, scoped, unscoped, cx, sink);

    // `...incoming` needs a proposed row to spread — only a bulk `create … from` upsert has
    // one; an inline `create` does not.
    if let Some(sp) = &oc.spread {
        sink.error_note(
            code::INPUT_INCOMING_CONTEXT,
            sp.span,
            "`...incoming` is only valid in a bulk `create … from … on conflict update` branch"
                .to_string(),
            "an inline `create` has no proposed row to spread — set the columns explicitly",
        );
    }

    // The update branch is an ordinary update — check its assigns — but it may not assign
    // a conflict column (moving the key would break the conflict + the read-back).
    let no_bindings = Bindings::default();
    for a in &oc.update {
        check_assign(
            a,
            mi,
            cx,
            params,
            &no_bindings,
            /* in_update = */ true,
            sink,
        );
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
    }
}
