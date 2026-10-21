use super::*;

/// Check `out = filter(args)` against the shape's model: the filter must exist with a
/// matching arity and its body must resolve here. A bare `$p` argument is accepted; each
/// query returning the shape binds it (`check_shape_params`).
pub(super) fn check_filter_projection(fp: &FilterProjection, mi: usize, cx: &Cx, sink: &mut Sink) {
    if !cx.filters.contains_key(&fp.name.node) {
        sink.error_note(
            code::UNKNOWN_FILTER,
            fp.name.span,
            format!("unknown filter `{}`", fp.name.node),
            format!(
                "a call in a shape value is a named filter or an aggregate ({})",
                KNOWN_AGGS.join(", ")
            ),
        );
        return;
    }
    let bound: Vec<String> = fp
        .args
        .iter()
        .filter_map(|a| match a {
            Value::Param(pr) if pr.name.node != "ctx" => Some(pr.name.node.clone()),
            _ => None,
        })
        .collect();
    for a in &fp.args {
        reject_optional_value(a, "a shape's filter argument", sink);
    }
    resolve::check_filter_call(
        &fp.name,
        &fp.args,
        Some(mi),
        cx,
        &bound,
        &mut Vec::new(),
        sink,
    );
}
