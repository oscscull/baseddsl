use super::*;

/// Every left operand in a `having` predicate must name a projected column of the
/// aggregate shape (an aggregate alias or a group column), so it maps to something the
/// grouped result actually has.
pub(super) fn check_having(
    p: &Predicate,
    out_names: &[String],
    params: &[String],
    qspan: Span,
    sink: &mut Sink,
) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            check_having(a, out_names, params, qspan, sink);
            check_having(b, out_names, params, qspan, sink);
        }
        Predicate::Not(inner) => check_having(inner, out_names, params, qspan, sink),
        Predicate::Cmp { path, value, .. } => {
            check_having_path(path, out_names, qspan, sink);
            if let Value::Param(pr) = value {
                resolve::check_param_ref(pr, params, sink);
            }
        }
        Predicate::InList { path, values } => {
            check_having_path(path, out_names, qspan, sink);
            for v in values {
                if let Value::Param(pr) = v {
                    resolve::check_param_ref(pr, params, sink);
                }
            }
        }
        Predicate::Bare(path) => check_having_path(path, out_names, qspan, sink),
        // A raw predicate term / named-filter call is a leaf escape — left unchecked.
        Predicate::FilterCall { .. } | Predicate::Raw(_) => {}
    }
}

fn check_having_path(path: &Path, out_names: &[String], qspan: Span, sink: &mut Sink) {
    let ok = path.segments.len() == 1 && out_names.contains(&path.segments[0].node);
    if !ok {
        let span = path.segments.last().map_or(qspan, |s| s.span);
        sink.error_note(
            code::AGG_GROUP_BY,
            span,
            format!(
                "`having` references `{}`, which the aggregate shape doesn't project",
                join_path(path)
            ),
            "filter on a projected aggregate alias or group column",
        );
    }
}
