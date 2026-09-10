use super::*;

struct CtxUseMode {
    optional: bool,
    required: bool,
    span: Span,
}

/// A query's optional-context reads: allowed in a `where`, but a field read both optional and
/// required in the same callable is an error, and an optional `?` is valid only on a
/// `$ctx.<field>`.
pub(crate) fn check_optional_ctx_query(q: &Query, sink: &mut Sink) {
    let clauses: &[Clause] = match &q.body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        QueryBody::Bare | QueryBody::Raw(_) => &[],
    };
    let mut modes: std::collections::HashMap<String, CtxUseMode> = std::collections::HashMap::new();
    for clause in clauses {
        if let Clause::Where(p) = clause {
            record_pred_ctx_modes(p, &mut modes, sink);
        }
    }
    for (field, m) in &modes {
        if m.optional && m.required {
            sink.error_note(
                code::OPT_CTX_MIXED,
                m.span,
                format!("`$ctx.{field}` is read both optional (`?`) and required in this query"),
                "an optional read widens its filter when the field is absent; a required read demands the value — pick one per callable",
            );
        }
    }
}

fn record_pred_ctx_modes(
    p: &Predicate,
    modes: &mut std::collections::HashMap<String, CtxUseMode>,
    sink: &mut Sink,
) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            record_pred_ctx_modes(a, modes, sink);
            record_pred_ctx_modes(b, modes, sink);
        }
        Predicate::Not(x) => record_pred_ctx_modes(x, modes, sink),
        Predicate::Cmp { value, .. } => record_value_ctx_mode(value, modes, sink),
        Predicate::InList { values, .. } => {
            for v in values {
                record_value_ctx_mode(v, modes, sink);
            }
        }
        // A named filter is shared with writes, so an optional read can't ride through it.
        Predicate::FilterCall { args, .. } => {
            for a in args {
                reject_optional_value(a, "a filter argument", sink);
            }
        }
        Predicate::Bare(_) | Predicate::Raw(_) => {}
    }
}

fn record_value_ctx_mode(
    v: &Value,
    modes: &mut std::collections::HashMap<String, CtxUseMode>,
    sink: &mut Sink,
) {
    let Value::Param(pr) = v else { return };
    if pr.name.node == "ctx" && pr.path.len() == 1 {
        let e = modes.entry(pr.path[0].node.clone()).or_insert(CtxUseMode {
            optional: false,
            required: false,
            span: pr.path[0].span,
        });
        if pr.optional {
            e.optional = true;
        } else {
            e.required = true;
        }
    } else if pr.optional {
        reject_optional_value(v, "a non-`$ctx` reference", sink);
    }
}
