use super::*;

pub(super) fn check_assign(
    a: &Assign,
    mi: usize,
    cx: &Cx,
    params: &[String],
    bindings: &Bindings,
    in_update: bool,
    sink: &mut Sink,
) {
    if reject_incoming_keyword(a, sink) {
        return;
    }
    let Some(member) = cx.model(mi).member(&a.col.node) else {
        unknown_field(cx, mi, &a.col, sink);
        return;
    };
    if reject_generated(member, a, sink) || reject_opaque(member, a, sink) {
        return;
    }
    // An arithmetic RHS (`total = total + $n`) is numeric-only and valid only in an
    // `update`; a create has no prior row to reference.
    let Some(value) = a.value.as_value() else {
        resolve::check_assign_arith(
            &a.value,
            &member.kind,
            &a.col,
            mi,
            in_update,
            cx,
            params,
            sink,
        );
        return;
    };
    // A `$name.field` that is neither `$ctx` nor a declared param is a `tx` step binding
    // (`$` unifies params and bindings); resolve it against the bound step's model.
    if let Value::Param(pr) = value {
        if pr.name.node != "ctx" && !params.iter().any(|p| p == &pr.name.node) {
            check_binding_ref(pr, bindings, &member.kind, &a.col, cx, sink);
            return;
        }
    }
    // An enum column is assigned a bare variant (`status = paid`); check variant membership.
    if let MemberKind::Scalar {
        enum_name: Some(en_name),
        ..
    } = &member.kind
    {
        if let Some(en) = cx.enum_(en_name) {
            if resolve::check_enum_operand(value, en, params, sink) {
                return;
            }
        }
    }
    resolve::check_value(value, Some(mi), cx, params, sink);
    // The value's type must match the target column (skipped if resolution already failed).
    resolve::check_assign_type(&member.kind, &a.col, value, mi, cx, sink);
}

/// `incoming.<col>` is the bulk-upsert keyword, legal only inside a `create … from …
/// on conflict update` branch (checked in `check_bulk_upsert`). Reject it in any other
/// assign context, before the path resolves as an unknown relation reach.
fn reject_incoming_keyword(a: &Assign, sink: &mut Sink) -> bool {
    let Some(span) = rhs_incoming_span(&a.value) else {
        return false;
    };
    sink.error_note(
        code::INPUT_INCOMING_CONTEXT,
        span,
        "`incoming.<col>` is only valid in a bulk `create … from … on conflict update` branch",
        "the incoming-row keyword names the proposed row of a bulk upsert; drop it here",
    );
    true
}

/// A generated column is derived by the database on write.
fn reject_generated(member: &RMember, a: &Assign, sink: &mut Sink) -> bool {
    if !member.kind.is_generated() {
        return false;
    }
    sink.error_note(
        code::GEN_ASSIGN,
        a.col.span,
        format!("cannot write `{}` — it is a generated column", a.col.node),
        "a generated column's value is derived from the row's own columns",
    );
    true
}

/// An opaque column is excluded from writes; the DB or a raw migration owns its value.
fn reject_opaque(member: &RMember, a: &Assign, sink: &mut Sink) -> bool {
    let Some(spec) = member.kind.opaque() else {
        return false;
    };
    sink.error_note(
        code::OPAQUE_ASSIGN,
        a.col.span,
        format!(
            "cannot write `{}` — a {} column is opaque",
            a.col.node,
            spec.render()
        ),
        "the engine does not model this type; set it from a raw migration or a DB default",
    );
    true
}
