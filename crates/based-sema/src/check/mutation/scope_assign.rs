use super::*;

/// On a scoped model the `@scope` column is engine-managed on `create`: it is auto-set
/// from `$ctx.<field>` so a caller can only plant a row in their own scope. Assigning it
/// is therefore an error, like assigning `id`/`@created` — *unless* the mutation is
/// `unscoped`, where scope injection is off and the caller owns the column. Reports every
/// offending assign.
pub(super) fn check_scope_assign(mi: usize, assigns: &[Assign], unscoped: bool, cx: &Cx, sink: &mut Sink) {
    if unscoped {
        return;
    }
    // Every alternative's columns are engine-domain, not just the chosen one:
    // planting *any* scope column plants the row into an arbitrary scope.
    let scope_cols = crate::scope::all_scope_cols(mi, cx);
    for a in assigns {
        if scope_cols.iter().any(|f| f == &a.col.node) {
            sink.error_note(
                code::SCOPE_ASSIGN,
                a.col.span,
                format!(
                    "`{}` is `@scope`-managed on `create`; the engine sets it from `$ctx`",
                    a.col.node
                ),
                "a scoped create can't target another scope — drop the assign, or mark the mutation `unscoped(\"…\")`",
            );
        }
    }
}
