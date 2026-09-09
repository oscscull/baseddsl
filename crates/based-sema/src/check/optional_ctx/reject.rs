use super::*;

/// Reject a stray optional `?` — every position but a query `where`'s `$ctx.<field>`.
pub(super) fn reject_optional_value(v: &Value, at: &str, sink: &mut Sink) {
    if let Value::Param(pr) = v {
        if pr.optional {
            let span = pr.path.last().map_or(pr.name.span, |s| s.span);
            sink.error_note(
                code::OPT_CTX_PLACEMENT,
                span,
                format!("optional `?` is not allowed on {at}"),
                "a trailing `?` marks an optional `$ctx.<field>` read, valid only in a query filter (auth.md Handle 1) — absent-means-widen must never touch a scope or a write",
            );
        }
    }
}

/// Reject every optional `?` in a predicate (a mutation filter or a named-filter body).
pub(crate) fn forbid_optional_in_pred(p: &Predicate, at: &str, sink: &mut Sink) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            forbid_optional_in_pred(a, at, sink);
            forbid_optional_in_pred(b, at, sink);
        }
        Predicate::Not(x) => forbid_optional_in_pred(x, at, sink),
        Predicate::Cmp { value, .. } => reject_optional_value(value, at, sink),
        Predicate::InList { values, .. } => {
            for v in values {
                reject_optional_value(v, at, sink);
            }
        }
        Predicate::FilterCall { args, .. } => {
            for a in args {
                reject_optional_value(a, at, sink);
            }
        }
        Predicate::Bare(_) | Predicate::Raw(_) => {}
    }
}

/// A mutation is a write; an optional context read has no place in any of its statements.
pub(crate) fn forbid_optional_ctx_writes(body: &[WriteStmt], sink: &mut Sink) {
    for stmt in body {
        match stmt {
            WriteStmt::Create {
                assigns, conflict, ..
            } => {
                for a in assigns {
                    reject_optional_assign(a, sink);
                }
                if let Some(oc) = conflict {
                    for a in &oc.update {
                        reject_optional_assign(a, sink);
                    }
                }
            }
            WriteStmt::Update {
                where_, assigns, ..
            } => {
                forbid_optional_in_pred(where_, "a mutation filter", sink);
                for a in assigns {
                    reject_optional_assign(a, sink);
                }
            }
            WriteStmt::Restore { where_, .. } => {
                forbid_optional_in_pred(where_, "a mutation filter", sink);
            }
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                if let Some(p) = where_ {
                    forbid_optional_in_pred(p, "a mutation filter", sink);
                }
            }
            WriteStmt::Tx(inner) => forbid_optional_ctx_writes(inner, sink),
            WriteStmt::Raw(_) => {}
        }
    }
}

fn reject_optional_assign(a: &Assign, sink: &mut Sink) {
    if let Some(v) = a.value.as_value() {
        reject_optional_value(v, "a write assignment", sink);
    }
}
