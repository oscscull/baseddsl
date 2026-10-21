use super::*;

/// The column filter `def`'s param `param` is first compared against when the filter
/// expands on model `model` — what a caller's argument must agree with.
/// `None` when the param only reaches untyped positions (a raw body, a function arg).
pub(crate) fn filter_param_column<'f>(
    def: &'f NamedFilter,
    param: &str,
    model: usize,
    cx: &Cx<'f>,
) -> Option<Terminal> {
    pred_param_column(
        &def.pred,
        param,
        model,
        cx,
        &mut vec![def.name.node.as_str()],
    )
}

fn pred_param_column<'f>(
    pred: &'f Predicate,
    param: &str,
    model: usize,
    cx: &Cx<'f>,
    in_filters: &mut Vec<&'f str>,
) -> Option<Terminal> {
    let names =
        |v: &Value| matches!(v, Value::Param(pr) if pr.path.is_empty() && pr.name.node == param);
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            pred_param_column(a, param, model, cx, in_filters)
                .or_else(|| pred_param_column(b, param, model, cx, in_filters))
        }
        Predicate::Not(p) => pred_param_column(p, param, model, cx, in_filters),
        Predicate::Cmp { path, value, .. } if names(value) => resolve_quiet(path, model, cx),
        Predicate::InList { path, values } if values.iter().any(names) => {
            resolve_quiet(path, model, cx)
        }
        Predicate::FilterCall { name, args } => {
            let inner = cx.filters.get(&name.node).copied()?;
            if in_filters.contains(&inner.name.node.as_str()) {
                return None;
            }
            in_filters.push(inner.name.node.as_str());
            let found = args
                .iter()
                .zip(&inner.params)
                .filter(|(a, _)| names(a))
                .find_map(|(_, p)| {
                    pred_param_column(&inner.pred, &p.name.node, model, cx, in_filters)
                });
            in_filters.pop();
            found
        }
        Predicate::Any {
            path,
            pred: Some(inner),
            ..
        } => {
            let target = resolve_any_target(path, model, cx, &mut Sink::default())?;
            pred_param_column(inner, param, target, cx, in_filters)
        }
        _ => None,
    }
}
