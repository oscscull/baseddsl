use super::*;

/// Resolve a relation's custom `on:` join predicate.
/// Unlike every other predicate this one spans *two* tables — the FK-holding
/// model `near` and its target `far` — and refers to columns table-qualified
/// (`orders.user_ref = users.legacy_id`), for legacy keys that don't follow the
/// `<field>_id` convention. Each column path must be `<table>.<column>` naming one
/// of the two tables in scope and a real column on it, so the join stays inside the
/// guarantee (the engine still understands and types it). A join is static
/// structure, so request `$`-params, filter calls, and `^` back-references have no
/// meaning here and are rejected; literals (a constant discriminator) are fine.
pub fn check_relation_on(pred: &Predicate, near: usize, far: usize, cx: &Cx, sink: &mut Sink) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            check_relation_on(a, near, far, cx, sink);
            check_relation_on(b, near, far, cx, sink);
        }
        Predicate::Not(p) => check_relation_on(p, near, far, cx, sink),
        Predicate::Cmp { path, value, .. } => {
            resolve_join_path(path, near, far, cx, sink);
            check_join_value(value, near, far, cx, sink);
        }
        Predicate::InList { path, values } => {
            resolve_join_path(path, near, far, cx, sink);
            for v in values {
                check_join_value(v, near, far, cx, sink);
            }
        }
        Predicate::Bare(path) => resolve_join_path(path, near, far, cx, sink),
        Predicate::FilterCall { name, .. } => sink.error(
            code::JOIN_FORM,
            name.span,
            "named filters aren't allowed in a custom `on:` join condition",
        ),
        // A raw join fragment is an escape hatch the engine can't resolve — leave it.
        Predicate::Raw(_) => {}
    }
}

/// One value in a custom `on:` join: a column resolves table-qualified, a literal
/// (a constant discriminator) is fine, and anything request-bound is rejected —
/// a join is static structure.
pub(crate) fn check_join_value(value: &Value, near: usize, far: usize, cx: &Cx, sink: &mut Sink) {
    match value {
        Value::Path(p) => resolve_join_path(p, near, far, cx, sink),
        Value::Lit(_) => {}
        Value::Param(pr) => sink.error(
            code::JOIN_FORM,
            pr.name.span,
            "a custom join is static structure — `$` params aren't bound in an `on:` condition",
        ),
        Value::Func(f) => sink.error(
            code::JOIN_FORM,
            f.name.span,
            "a custom join is static structure — functions aren't allowed in an `on:` condition",
        ),
    }
}

/// Resolve one table-qualified column path (`orders.user_ref`) in a custom join.
/// Must be exactly two segments: a table naming `near` or `far`, then a physical
/// column on that model. Self-ref joins (`near == far`) resolve against the one
/// model on either side; distinguishing the two logical sides is a codegen alias
/// concern, not a resolution one.
pub(crate) fn resolve_join_path(path: &Path, near: usize, far: usize, cx: &Cx, sink: &mut Sink) {
    let Some(last) = path.segments.last() else {
        return;
    };
    if path.segments.len() != 2 {
        sink.error(
            code::JOIN_FORM,
            last.span,
            "custom-join column must be table-qualified `<table>.<column>`",
        );
        return;
    }
    let table = &path.segments[0];
    let col = &path.segments[1];
    let mi = if table.node == cx.model(near).table {
        near
    } else if table.node == cx.model(far).table {
        far
    } else {
        sink.error(
            code::JOIN_TABLE,
            table.span,
            format!(
                "unknown table `{}` in join (expected `{}` or `{}`)",
                table.node,
                cx.model(near).table,
                cx.model(far).table
            ),
        );
        return;
    };
    if cx.model(mi).column(&col.node).is_none() {
        sink.error(
            code::UNKNOWN_FIELD,
            col.span,
            format!(
                "table `{}` has no column `{}`",
                cx.model(mi).table,
                col.node
            ),
        );
    }
}
