use super::*;

/// Check the body's clauses against the target model. Returns whether the body carries
/// its own `order` clause.
pub(super) fn check_query_body(
    q: &Query,
    ti: usize,
    agg_body: Option<&[ShapeField]>,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) -> bool {
    let (clauses, mi) = match &q.body {
        QueryBody::Bare => return false,
        QueryBody::Raw(raw) => {
            check_raw_query(q, raw, ti, cx, params, sink);
            return false;
        }
        QueryBody::Inline(clauses) => (clauses.as_slice(), ti),
        QueryBody::Block(s) => (s.clauses.as_slice(), cx.find(&s.model.node).unwrap_or(ti)),
    };
    if let Some(body) = agg_body {
        check_agg_query(q, clauses, mi, body, cx, params, sink);
        false
    } else {
        reject_agg_clauses(q, clauses, sink);
        check_clauses(clauses, mi, cx, params, sink)
    }
}
