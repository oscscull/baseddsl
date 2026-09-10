use super::*;

/// Type-check one `create`/`update` assignment: the value's family must agree with
/// the target column's — the write-side twin of the `=` compatibility rule
/// (`check_cmp_types`). A literal or another column is family-checked. Params (typed at
/// their declaration / `$ctx` inferred) and functions (return type unmodelled) are
/// skipped, exactly as on the read side. Silent when a side fails to resolve — that
/// name error is already reported by the caller. A `$step.field` tx binding reference is
/// checked separately (`check::check_binding_ref` → `check_field_assign_type`).
pub fn check_assign_type(
    target: &MemberKind,
    col: &Ident,
    value: &Value,
    mi: usize,
    cx: &Cx,
    sink: &mut Sink,
) {
    let Some(lf) = member_family(target) else {
        return;
    };
    let rf = match value {
        Value::Lit(l) => match lit_family(l) {
            Some(f) => f,
            None => return, // null: no constraint
        },
        Value::Path(p) => match resolve_quiet(p, mi, cx) {
            Some(t) => terminal_family(&t),
            None => return, // unresolved column: name error already reported
        },
        Value::Param(_) | Value::Func(_) => return,
    };
    report_assign_family(target, col, rf, lf, sink);
}

/// Family-check a `$step.field` tx binding assignment: the bound field's family
/// must agree with the target column's, the same rule `check_assign_type` applies to a
/// column/literal RHS. Silent when either side has no modellable family.
pub fn check_field_assign_type(
    target: &MemberKind,
    col: &Ident,
    source: &MemberKind,
    sink: &mut Sink,
) {
    let (Some(lf), Some(rf)) = (member_family(target), member_family(source)) else {
        return;
    };
    report_assign_family(target, col, rf, lf, sink);
}

/// Emit when an assigned value's family (`rf`) is incompatible with the target
/// column's (`lf`). Shared by the column/literal and the tx-binding assign checks.
pub(crate) fn report_assign_family(target: &MemberKind, col: &Ident, rf: Family, lf: Family, sink: &mut Sink) {
    if !compatible(lf, rf) {
        let target_desc = match target {
            MemberKind::Scalar { ty, .. } => format!("`{}`", prim_name(*ty)),
            MemberKind::Forward { target, .. } => format!("relation `{target}`"),
            MemberKind::Inverse { .. } => return,
        };
        sink.error(
            code::ASSIGN_TYPE,
            col.span,
            format!(
                "cannot assign a {} value to `{}` (a {} column)",
                family_name(rf),
                col.node,
                target_desc
            ),
        );
    }
}

/// Type-check an arithmetic assignment RHS (`total = total + $n`,.
/// Numeric-only and update-only: a `create` has no existing row to self-reference
///; every column operand must be numeric; and, via the ordinary
/// assign-type rule, the target column must be numeric too. Params and
/// functions are typed at their declaration / unmodelled, so they are skipped here —
/// exactly as on every other write-side family check.
#[allow(clippy::too_many_arguments)]
pub fn check_assign_arith(
    rhs: &AssignRhs,
    target: &MemberKind,
    col: &Ident,
    mi: usize,
    in_update: bool,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    if !in_update {
        if let AssignRhs::Arith { span, .. } = rhs {
            sink.error(
                code::ARITH_CREATE,
                *span,
                "an arithmetic expression needs an existing row — valid only in `update`, \
                 not `create`"
                    .to_string(),
            );
        }
        return;
    }
    if member_family(target) != Some(Family::Numeric) {
        let target_desc = match target {
            MemberKind::Scalar { ty, .. } => format!("a `{}` column", prim_name(*ty)),
            MemberKind::Forward { target, .. } => format!("relation `{target}`"),
            MemberKind::Inverse { .. } => return,
        };
        sink.error(
            code::ASSIGN_TYPE,
            col.span,
            format!(
                "cannot assign an arithmetic (numeric) expression to `{}` ({target_desc})",
                col.node
            ),
        );
    }
    check_arith_operands(rhs, col, mi, cx, params, sink);
}

/// Walk an arithmetic RHS: resolve each leaf value (names, params, back-refs) and
/// require every column / literal operand to be numeric.
pub(crate) fn check_arith_operands(
    rhs: &AssignRhs,
    col: &Ident,
    mi: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    match rhs {
        AssignRhs::Arith { lhs, rhs, .. } => {
            check_arith_operands(lhs, col, mi, cx, params, sink);
            check_arith_operands(rhs, col, mi, cx, params, sink);
        }
        AssignRhs::Value(v) => {
            check_value(v, Some(mi), cx, params, sink);
            match v {
                Value::Path(p) => {
                    if let Some(t) = resolve_quiet(p, mi, cx) {
                        if terminal_family(&t) != Family::Numeric {
                            if let Some(seg) = p.segments.last() {
                                sink.error(
                                    code::ARITH_OPERAND,
                                    seg.span,
                                    format!(
                                        "{} is not numeric — an arithmetic update expression \
                                         takes int/float/decimal operands",
                                        terminal_name(&t)
                                    ),
                                );
                            }
                        }
                    }
                }
                // A literal carries no span, so a non-numeric one (`total + "x"`) is
                // anchored at the assignment target, like the assign-type error.
                Value::Lit(l) => {
                    if matches!(lit_family(l), Some(f) if f != Family::Numeric) {
                        sink.error(
                            code::ARITH_OPERAND,
                            col.span,
                            "a non-numeric literal in an arithmetic update expression".to_string(),
                        );
                    }
                }
                Value::Param(_) | Value::Func(_) => {}
            }
        }
    }
}
