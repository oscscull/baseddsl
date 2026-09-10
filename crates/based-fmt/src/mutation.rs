//! Reprint a single write statement (create/update/delete/restore/hard delete/raw),
//! its assignment block, an `on conflict … update` branch, and the start-byte a
//! comment anchors to for a write.

use based_ast::*;

use crate::*;

pub(crate) fn write_line(w: &WriteStmt) -> String {
    match w {
        WriteStmt::Create {
            model,
            assigns,
            from,
            conflict,
            binding,
        } => {
            let mut s = match from {
                Some(cf) => format!(
                    "create {}{} from ${}",
                    model.node,
                    if cf.bulk { "[]" } else { "" },
                    cf.param.node
                ),
                None => format!("create {} {}", model.node, assign_block(assigns)),
            };
            if let Some(oc) = conflict {
                s.push_str(&format!(
                    " on conflict ({}) update {}",
                    oc.target
                        .iter()
                        .map(|t| t.node.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                    conflict_update_block(oc)
                ));
            }
            if let Some(b) = binding {
                s.push_str(&format!(" as {}", b.node));
            }
            s
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => format!(
            "update {} where ({}) {}",
            model.node,
            predicate(where_, 0),
            assign_block(assigns)
        ),
        WriteStmt::Delete { model, where_ } => match where_ {
            Some(p) => format!("delete {} where ({})", model.node, predicate(p, 0)),
            None => format!("delete all {}", model.node),
        },
        WriteStmt::Restore { model, where_ } => {
            format!("restore {} where ({})", model.node, predicate(where_, 0))
        }
        WriteStmt::HardDelete { model, where_ } => match where_ {
            Some(p) => format!("hard delete {} where ({})", model.node, predicate(p, 0)),
            None => format!("hard delete all {}", model.node),
        },
        WriteStmt::Raw(r) => raw_sql(r),
        // Tx is handled by the caller (it spans multiple lines).
        WriteStmt::Tx(_) => String::new(),
    }
}

/// An `on conflict … update { … }` branch: the explicit assigns plus a `...incoming` spread
/// reprinted at its original lexical position (bulk upsert).
fn conflict_update_block(oc: &OnConflict) -> String {
    let mut parts: Vec<String> = oc
        .update
        .iter()
        .map(|a| format!("{} = {}", a.col.node, assign_rhs(&a.value)))
        .collect();
    if let Some(sp) = &oc.spread {
        parts.insert(
            sp.preceding.min(parts.len()),
            format!("...{}", sp.source.node),
        );
    }
    if parts.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", parts.join(", "))
    }
}

fn assign_block(assigns: &[Assign]) -> String {
    if assigns.is_empty() {
        return "{}".to_string();
    }
    format!(
        "{{ {} }}",
        assigns
            .iter()
            .map(|a| format!("{} = {}", a.col.node, assign_rhs(&a.value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A representative start byte offset for a write statement — its model ident (or the raw
/// span / first inner write), used to place body comments relative to it.
pub(crate) fn write_start(w: &WriteStmt) -> Option<u32> {
    match w {
        WriteStmt::Create { model, .. }
        | WriteStmt::Update { model, .. }
        | WriteStmt::Delete { model, .. }
        | WriteStmt::Restore { model, .. }
        | WriteStmt::HardDelete { model, .. } => Some(model.span.start),
        WriteStmt::Raw(r) => Some(r.span.start),
        WriteStmt::Tx(inner) => inner.first().and_then(write_start),
    }
}
