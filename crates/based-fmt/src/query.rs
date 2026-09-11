//! Reprint a read statement and its parts: the verb head, individual clauses
//! (where/order/page/group by/having/unindexed), the return type, the `for update`
//! modifier, and the parameter list.

use crate::*;

pub(crate) fn statement_inline(stmt: &Statement) -> String {
    let mut s = format!("{} {}", verb_head(stmt), stmt.model.node);
    for c in &stmt.clauses {
        s.push(' ');
        s.push_str(&clause(c));
    }
    if let Some(wait) = stmt.for_update {
        s.push(' ');
        s.push_str(for_update_modifier(wait));
    }
    s.push(';');
    s
}

/// The `for update` locking modifier, with its optional wait mode.
pub(crate) fn for_update_modifier(wait: LockWait) -> &'static str {
    match wait {
        LockWait::Wait => "for update",
        LockWait::NoWait => "for update nowait",
        LockWait::SkipLocked => "for update skip locked",
    }
}

fn verb(v: Verb) -> &'static str {
    match v {
        Verb::Get => "get",
        Verb::List => "list",
    }
}

/// The statement head: the verb, plus `distinct` on a `list distinct` read.
pub(crate) fn verb_head(stmt: &Statement) -> String {
    if stmt.distinct {
        format!("{} distinct", verb(stmt.verb))
    } else {
        verb(stmt.verb).to_string()
    }
}

pub(crate) fn clause(c: &Clause) -> String {
    match c {
        Clause::Where(p) => format!("where ({})", predicate(p, 0)),
        Clause::Order(terms) => format!(
            "order ({})",
            terms.iter().map(sort_term).collect::<Vec<_>>().join(", ")
        ),
        Clause::Page(pc) => {
            let mut s = format!("page ({})", pc.size);
            if pc.offset {
                s.push_str(" offset");
            }
            if pc.with_count {
                s.push_str(" with count");
            }
            s
        }
        Clause::Unindexed(u) => match &u.kind {
            UnindexedKind::MaxRows(n) => format!("unindexed(max_rows: {n})"),
            UnindexedKind::Unsafe(None) => "unindexed(unsafe)".to_string(),
            UnindexedKind::Unsafe(Some(r)) => format!("unindexed(unsafe, \"{}\")", esc(r)),
        },
        Clause::GroupBy(cols) => format!(
            "group by ({})",
            cols.iter().map(path).collect::<Vec<_>>().join(", ")
        ),
        Clause::Having(p) => format!("having ({})", predicate(p, 0)),
    }
}

pub(crate) fn ret_type(r: &RetType) -> String {
    format!(
        "{}{}{}",
        if r.stream { "stream " } else { "" },
        r.ty.node,
        if r.many { "[]" } else { "" }
    )
}

pub(crate) fn params(ps: &[Param]) -> String {
    ps.iter().map(param).collect::<Vec<_>>().join(", ")
}

fn param(p: &Param) -> String {
    let mut s = p.name.node.clone();
    if p.optional {
        s.push('?');
    }
    if let Some(ty) = &p.ty {
        s.push_str(&format!(": {}", type_expr(ty)));
    }
    match &p.binding {
        Some(ParamBinding::Edge(e)) => s.push_str(&format!(" -> {}", e.node)),
        Some(ParamBinding::ColOp { op, col }) => {
            s.push_str(&format!(" {} {}", op_str(*op), col.node));
        }
        None => {}
    }
    if let Some(d) = &p.default {
        s.push_str(&format!(" = {}", default_val(d)));
    }
    s
}
