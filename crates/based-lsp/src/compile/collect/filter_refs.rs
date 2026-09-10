use super::*;

/// Every `filter(...)` call-name identifier across the AST — the sites a name *invokes*
/// a declared filter (`Predicate::FilterCall`), found by walking every predicate a
/// query/mutation/filter carries. The reference-collection twin for filters.
pub(crate) fn collect_filter_refs(decls: &[Decl]) -> Vec<&Ident> {
    let mut out = Vec::new();
    for d in decls {
        match d {
            Decl::Query(q) => match &q.body {
                QueryBody::Inline(cs) => cs.iter().for_each(|c| clause_filter_refs(c, &mut out)),
                QueryBody::Block(stmt) => stmt
                    .clauses
                    .iter()
                    .for_each(|c| clause_filter_refs(c, &mut out)),
                QueryBody::Bare | QueryBody::Raw(_) => {}
            },
            Decl::Mutation(m) => write_filter_refs(&m.body, &mut out),
            Decl::Filter(f) => pred_filter_refs(&f.pred, &mut out),
            _ => {}
        }
    }
    out
}

/// Filter-call names in a query clause's `where` predicate.
pub(crate) fn clause_filter_refs<'a>(c: &'a Clause, out: &mut Vec<&'a Ident>) {
    if let Clause::Where(p) = c {
        pred_filter_refs(p, out);
    }
}

/// Filter-call names in a mutation write body's `where` predicates (recursing `tx`).
pub(crate) fn write_filter_refs<'a>(body: &'a [WriteStmt], out: &mut Vec<&'a Ident>) {
    for w in body {
        match w {
            WriteStmt::Update { where_, .. } | WriteStmt::Restore { where_, .. } => {
                pred_filter_refs(where_, out);
            }
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                if let Some(p) = where_ {
                    pred_filter_refs(p, out);
                }
            }
            WriteStmt::Tx(inner) => write_filter_refs(inner, out),
            WriteStmt::Create { .. } | WriteStmt::Raw(_) => {}
        }
    }
}

/// Filter-call names anywhere in a predicate tree.
pub(crate) fn pred_filter_refs<'a>(p: &'a Predicate, out: &mut Vec<&'a Ident>) {
    match p {
        Predicate::Or(a, b) | Predicate::And(a, b) => {
            pred_filter_refs(a, out);
            pred_filter_refs(b, out);
        }
        Predicate::Not(inner) => pred_filter_refs(inner, out),
        Predicate::FilterCall { name, .. } => out.push(name),
        Predicate::Cmp { .. }
        | Predicate::InList { .. }
        | Predicate::Bare(_)
        | Predicate::Raw(_) => {}
    }
}
