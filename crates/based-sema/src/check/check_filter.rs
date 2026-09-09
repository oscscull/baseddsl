use super::*;

pub(crate) fn check_filter(f: &NamedFilter, cx: &Cx, sink: &mut Sink) -> RFilter {
    let params: Vec<String> = f.params.iter().map(|p| p.name.node.clone()).collect();
    for p in &f.params {
        if let Some(d) = &p.default {
            resolve::check_default(d, sink);
        }
    }
    // A named filter has no caller model at declaration, so column paths are not
    // bound here (they resolve against whichever model calls it) — only params,
    // nested filter calls, and functions are checked.
    resolve::check_predicate(&f.pred, None, cx, &params, sink);
    // A filter body is spliced into reads *and* writes, so an optional read can't live here.
    forbid_optional_in_pred(&f.pred, "a named filter (shared with writes)", sink);
    RFilter {
        name: f.name.node.clone(),
        span: f.span,
        arity: f.params.len(),
    }
}
