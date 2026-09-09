use super::*;

/// The query's verb: explicit in a block body, else inferred from return cardinality.
/// Reports a block statement that reads a model the return type doesn't project.
pub(super) fn query_verb(q: &Query, ret_model: &str, sink: &mut Sink) -> Verb {
    match &q.body {
        QueryBody::Block(s) => {
            if s.model.node != ret_model {
                sink.error(
                    code::RETURN_MODEL_MISMATCH,
                    s.model.span,
                    format!(
                        "statement reads `{}` but the return type is from `{ret_model}`",
                        s.model.node
                    ),
                );
            }
            s.verb
        }
        _ if q.ret.many || q.ret.stream => Verb::List,
        _ => Verb::Get,
    }
}
