use super::*;

/// Type-check a computed shape-field expression (`out = price - discount` / `a || b` /
/// `case …`) against its shape's model, emitting the E032x diagnostics. Operands must be
/// reachable scalar columns or literals: an arithmetic operand must be numeric, a
/// concat operand text, a CASE's branches must unify; a `$…` operand is an
/// error because a shape carries no parameters. The CASE `when` reuses the shared
/// predicate checker (comparison-type errors included).
pub fn check_shape_expr(expr: &ShapeExpr, mi: usize, cx: &Cx, sink: &mut Sink) {
    infer_shape_expr(expr, mi, cx, sink);
}

/// Infer a computed expression's coarse value family while type-checking it. `None` = a
/// `null` literal or an unmodelled leaf (function / rejected operand) — compatible with any
/// context, so it never triggers a mismatch on its own.
pub(crate) fn infer_shape_expr(
    expr: &ShapeExpr,
    mi: usize,
    cx: &Cx,
    sink: &mut Sink,
) -> Option<Family> {
    match expr {
        ShapeExpr::Value(v) => infer_operand(v, mi, cx, sink),
        ShapeExpr::Arith { lhs, rhs, span, .. } => {
            require_family(
                lhs,
                Family::Numeric,
                code::CFIELD_ARITH_OPERAND,
                *span,
                mi,
                cx,
                sink,
            );
            require_family(
                rhs,
                Family::Numeric,
                code::CFIELD_ARITH_OPERAND,
                *span,
                mi,
                cx,
                sink,
            );
            Some(Family::Numeric)
        }
        ShapeExpr::Concat { lhs, rhs, span } => {
            require_family(
                lhs,
                Family::Textual,
                code::CFIELD_CONCAT_OPERAND,
                *span,
                mi,
                cx,
                sink,
            );
            require_family(
                rhs,
                Family::Textual,
                code::CFIELD_CONCAT_OPERAND,
                *span,
                mi,
                cx,
                sink,
            );
            Some(Family::Textual)
        }
        ShapeExpr::Case { arms, else_, span } => {
            for arm in arms {
                check_case_when(&arm.when, mi, cx, sink);
            }
            let mut result: Option<Family> = None;
            let mut mismatch = false;
            for branch in arms
                .iter()
                .map(|a| &a.then)
                .chain(std::iter::once(&**else_))
            {
                if let Some(f) = infer_shape_expr(branch, mi, cx, sink) {
                    match result {
                        None => result = Some(f),
                        Some(r) if r == f => {}
                        Some(_) => mismatch = true,
                    }
                }
            }
            if mismatch {
                sink.error_note(
                    code::CFIELD_CASE_MISMATCH,
                    *span,
                    "a `case`'s branches have different types",
                    "every `then`/`else` branch must produce the same kind of value",
                );
            }
            result
        }
    }
}

/// A CASE arm's `when` predicate. A shape has no parameters, so a `$…` in the condition is
/// an error; otherwise reuse the shared predicate checker for column resolution and
/// comparison-type checks.
pub(crate) fn check_case_when(pred: &Predicate, mi: usize, cx: &Cx, sink: &mut Sink) {
    if reject_pred_params(pred, sink) {
        return;
    }
    check_predicate(pred, Some(mi), cx, &[], sink);
}
