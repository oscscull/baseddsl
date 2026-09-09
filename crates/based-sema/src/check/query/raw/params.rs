use super::*;

/// A raw body's params must be typed bind values: there is no column to infer a type from,
/// and no engine-built WHERE for a binding to ride. `${ctx.…}` has no type source at all.
pub(super) fn check_raw_query_params(q: &Query, raw: &RawSql, params: &[String], sink: &mut Sink) {
    for p in &q.params {
        if p.ty.is_none() {
            sink.error_note(
                code::RAW_QUERY_PARAM,
                p.name.span,
                format!(
                    "param `{}` of raw-bodied query `{}` needs a type annotation",
                    p.name.node, q.name.node
                ),
                "a raw body gives no column to infer the type from",
            );
        }
        if p.binding.is_some() {
            sink.error_note(
                code::RAW_QUERY_PARAM,
                p.name.span,
                format!(
                    "param `{}` of raw-bodied query `{}` can't carry a binding",
                    p.name.node, q.name.node
                ),
                "the raw SQL is the whole filter — reference the param as `${…}` inside it",
            );
        }
    }
    for part in &raw.parts {
        if let RawPart::Param(pr) = part {
            if pr.name.node == "ctx" {
                sink.error_note(
                    code::RAW_QUERY_CTX,
                    pr.name.span,
                    "`${ctx.…}` in a raw query body has no type source",
                    "declare a typed param and pass the context value through it",
                );
            } else {
                resolve::check_param_ref(pr, params, sink);
            }
        }
    }
}
