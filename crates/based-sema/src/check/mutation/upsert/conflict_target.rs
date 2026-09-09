use super::*;

/// The conflict target must name a key the database actually enforces, every column of it
/// must get a value on the create, and — on a scoped model — it must include the scope
/// columns, else a conflict could match (and the update modify) another scope's row.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_conflict_target(
    oc: &OnConflict,
    target: &[&str],
    mi: usize,
    create_assigns: &[Assign],
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let set_cols: Vec<String> = create_assigns.iter().map(|a| a.col.node.clone()).collect();
    check_conflict_target_over(oc, target, mi, &set_cols, scoped, unscoped, cx, sink);
}

/// The conflict-target rules, parameterized by the columns the create *sets* — an inline
/// create's assigns, or a `create … from` input shape's covered columns — so both the
/// inline and bulk upsert paths validate the target identically.
#[allow(clippy::too_many_arguments)]
pub(in crate::check::mutation) fn check_conflict_target_over(
    oc: &OnConflict,
    target: &[&str],
    mi: usize,
    set_cols: &[String],
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let m = cx.model(mi);
    let scope_cols: Vec<String> = crate::scope::resolve_inject(scoped, unscoped, &[mi], cx)
        .into_iter()
        .flat_map(|si| si.terms)
        .map(|(field, _)| field)
        .collect();

    reject_upsert_soft_delete(oc, m, sink);
    reject_non_unique_target(oc, target, m, sink);
    check_target_columns_set(oc, set_cols, &scope_cols, sink);
    check_scope_columns_targeted(oc, target, &scope_cols, m, sink);
}

/// A tombstoned row still occupies its unique key, so a conflict would update the tombstone.
fn reject_upsert_soft_delete(oc: &OnConflict, m: &RModel, sink: &mut Sink) {
    if m.soft_delete.is_some() {
        sink.error_note(
            code::UPSERT_SOFT_DELETE,
            oc.span,
            format!(
                "`on conflict` is not allowed on the @soft_delete model `{}`",
                m.name
            ),
            "a tombstoned row still holds its unique key — an upsert would update it, not insert",
        );
    }
}

/// The target must be a key the database enforces.
fn reject_non_unique_target(oc: &OnConflict, target: &[&str], m: &RModel, sink: &mut Sink) {
    if !is_unique_key(m, target) {
        sink.error_note(
            code::UPSERT_TARGET,
            oc.span,
            format!(
                "conflict target ({}) is not a unique key of `{}`",
                target.join(", "),
                m.name
            ),
            "name a `(unique)` column, a `@index (…) unique`, or the pk — a conflict needs a key the database enforces",
        );
    }
}

/// Every conflict column must get a value on the create — assigned, or supplied by a
/// managed `@scope` column — so the conflict has something to match on.
fn check_target_columns_set(
    oc: &OnConflict,
    set_cols: &[String],
    scope_cols: &[String],
    sink: &mut Sink,
) {
    let assigned: Vec<&str> = set_cols.iter().map(String::as_str).collect();
    for t in &oc.target {
        let set = assigned.contains(&t.node.as_str()) || scope_cols.iter().any(|c| c == &t.node);
        if !set {
            sink.error_note(
                code::UPSERT_TARGET_UNSET,
                t.span,
                format!("conflict column `{}` is not set by the create", t.node),
                "assign it in the create block (or let a `@scope` column supply it) so the conflict has a value",
            );
        }
    }
}

/// On a scoped model the target must include the scope columns, so a conflict can only
/// match within the caller's own scope.
fn check_scope_columns_targeted(
    oc: &OnConflict,
    target: &[&str],
    scope_cols: &[String],
    m: &RModel,
    sink: &mut Sink,
) {
    for sc in scope_cols {
        if !target.iter().any(|t| t == sc) {
            sink.error_note(
                code::UPSERT_SCOPE,
                oc.span,
                format!(
                    "conflict target on scoped `{}` omits the scope column `{sc}`",
                    m.name
                ),
                "add it to the conflict target so a conflict can only match a row in the caller's own scope",
            );
        }
    }
}

/// Whether `target` (field names) is a declared unique key of `m`: a single `(unique)`
/// column (or the pk `id`, always unique), or a `@index (…) unique` whose columns are
/// exactly the set (order-insensitive).
fn is_unique_key(m: &RModel, target: &[&str]) -> bool {
    if target.len() == 1 && m.is_unique(target[0]) {
        return true;
    }
    m.indexes.iter().any(|ix| {
        ix.unique
            && ix.columns.len() == target.len()
            && ix.columns.iter().all(|c| target.contains(&c.as_str()))
    })
}
