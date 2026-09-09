use super::*;
use std::collections::HashSet;

/// Named step bindings (`create … as name`): a binding reaches any *prior* step. Pre-scan
/// every binding name in this tx so a forward reference is distinguishable from a plain
/// unbound name; then descend, growing the reachable set after each bound create and
/// flagging a binding that shadows a param or duplicates another.
pub(super) fn check_tx(
    inner: &[WriteStmt],
    cx: &Cx,
    params: &[String],
    bindings: &Bindings,
    scoped: Option<&Scoped>,
    unscoped: bool,
    sink: &mut Sink,
) {
    let mut binds = bindings.clone();
    for s in inner {
        if let WriteStmt::Create {
            binding: Some(b), ..
        } = s
        {
            binds.all.insert(b.node.clone());
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for s in inner {
        check_write(s, cx, params, &binds, scoped, unscoped, sink);
        let WriteStmt::Create {
            model,
            binding: Some(b),
            ..
        } = s
        else {
            continue;
        };
        if params.iter().any(|p| p == &b.node) {
            sink.error_note(
                code::BINDING_SHADOW,
                b.span,
                format!("step binding `{}` shadows a parameter", b.node),
                "rename the binding — `$…` must name one thing",
            );
        } else if !seen.insert(b.node.as_str()) {
            sink.error(
                code::BINDING_SHADOW,
                b.span,
                format!("duplicate step binding `{}` in this `tx`", b.node),
            );
        }
        if let Some(mi) = write_model(model, cx, &mut Sink::default()) {
            binds.resolved.insert(b.node.clone(), mi);
        }
    }
}

/// The `tx` step bindings in scope while checking a write. `resolved` maps a
/// binding name reachable *now* (a create at a prior step, `create … as name`) to its
/// model; `all` is every binding name declared anywhere in the enclosing `tx`, so a
/// forward reference (`$x` used before its `as x`) reads distinctly from a plain typo.
#[derive(Clone, Default)]
pub(super) struct Bindings {
    resolved: std::collections::HashMap<String, usize>,
    all: std::collections::HashSet<String>,
}

/// Resolve a `$name.field` reference to a `tx` step binding: `name` must name a binding
/// reachable from here (a prior step), the reference is field-access only (one segment),
/// and `field` must be a column of the bound step's model, its family agreeing with the
/// assigned column. An unbound / forward-referenced name and a malformed (multi-segment)
/// reference are both rejected.
pub(super) fn check_binding_ref(
    pr: &ParamRef,
    bindings: &Bindings,
    target: &MemberKind,
    col: &Ident,
    cx: &Cx,
    sink: &mut Sink,
) {
    let name = &pr.name.node;
    let Some(&mi) = bindings.resolved.get(name) else {
        let msg = if bindings.all.contains(name) {
            format!("`${name}` is bound by a later step — a step binding reaches only prior steps")
        } else {
            format!("`${name}` is not a parameter or a bound step (`create … as {name}`)")
        };
        sink.error(code::BINDING_UNBOUND, pr.name.span, msg);
        return;
    };
    // A bare `$name` (no field) referencing a composite-key create's whole row: its key
    // parts are the multi-column FK's values, pulled per part from the bound create. Valid
    // only when assigning a relation *into* that same composite-key model.
    if pr.path.is_empty() {
        if let MemberKind::Forward { target: tgt, .. } = target {
            let bound = cx.model(mi);
            if bound.is_composite_key() && &bound.name == tgt {
                return;
            }
        }
    }
    let [field] = pr.path.as_slice() else {
        sink.error(
            code::BINDING_UNBOUND,
            pr.name.span,
            format!("reference a bound step's field as `${name}.field` (e.g. `${name}.id`)"),
        );
        return;
    };
    let Some(member) = cx.model(mi).member(&field.node) else {
        unknown_field(cx, mi, field, sink);
        return;
    };
    resolve::check_field_assign_type(target, col, &member.kind, sink);
}
