use super::*;

/// Validate one aggregate call against its shape's model: the function must be known, its
/// argument arity must match (`count()` takes none, the rest one), and the aggregated
/// column must be an eligible type.
pub(crate) fn check_agg_call(agg: &AggCall, mi: usize, cx: &Cx, sink: &mut Sink) {
    let func = agg.func.node.as_str();
    if !KNOWN_AGGS.contains(&func) {
        sink.error(
            code::AGG_CALL,
            agg.func.span,
            format!(
                "unknown aggregate `{func}` (expected one of: {})",
                KNOWN_AGGS.join(", ")
            ),
        );
        return;
    }
    match (func, &agg.arg) {
        ("count", Some(_)) => sink.error_note(
            code::AGG_CALL,
            agg.span,
            "`count` takes no argument".to_string(),
            "`count()` counts rows in the group",
        ),
        ("count", None) => {}
        (_, None) => sink.error(
            code::AGG_CALL,
            agg.span,
            format!("`{func}` needs one column argument, e.g. `{func}(total)`"),
        ),
        (_, Some(arg)) => {
            if let Some(term) = resolve::resolve_path(arg, mi, cx, sink) {
                if resolve::reject_opaque(&term, arg, "aggregate", sink) {
                    return;
                }
                let is_enum = cx.terminal_enum(arg, mi).is_some();
                if let Some(reason) = resolve::agg_operand_reason(func, &term, is_enum) {
                    let span = arg.segments.last().map_or(agg.span, |s| s.span);
                    sink.error(code::AGG_OPERAND, span, reason);
                }
            }
        }
    }
}
