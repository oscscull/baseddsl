use super::*;

/// A `get` is validly keyed if its equality-constrained columns cover some unique key: a
/// unique column, every part of a composite `@key`, or every column of a unique index.
pub(super) fn get_is_keyed(q: &Query, ti: usize, cx: &Cx) -> bool {
    if let QueryBody::Block(s) = &q.body {
        let sm = cx.model(cx.find(&s.model.node).unwrap_or(ti));
        let mut cols = Vec::new();
        for c in &s.clauses {
            if let Clause::Where(p) = c {
                collect_eq_cols(p, &mut cols);
            }
        }
        covers_unique_key(sm, &cols)
    } else {
        let cols: Vec<String> = q
            .params
            .iter()
            .filter_map(|p| match &p.binding {
                None => Some(p.name.node.clone()),
                Some(ParamBinding::Edge(e)) => Some(e.node.clone()),
                Some(ParamBinding::ColOp { op: Op::Eq, col }) => Some(col.node.clone()),
                _ => None,
            })
            .collect();
        covers_unique_key(cx.model(ti), &cols)
    }
}

/// The equality-constrained columns pin every column of at least one unique key, so the
/// query matches at most one row.
fn covers_unique_key(m: &RModel, eq_cols: &[String]) -> bool {
    let covered = |cols: &[String]| !cols.is_empty() && cols.iter().all(|k| eq_cols.contains(k));
    eq_cols.iter().any(|c| m.is_unique(c))
        || (m.is_composite_key() && covered(&m.key))
        || m.indexes.iter().any(|ix| ix.unique && covered(&ix.columns))
}

/// Collect single-segment columns constrained by equality in AND position. An equality
/// under `or` or `not` doesn't restrict every matching row, so it can't key the query.
fn collect_eq_cols(p: &Predicate, out: &mut Vec<String>) {
    match p {
        Predicate::And(a, b) => {
            collect_eq_cols(a, out);
            collect_eq_cols(b, out);
        }
        Predicate::Cmp {
            path, op: Op::Eq, ..
        } if path.segments.len() == 1 => {
            out.push(path.segments[0].node.clone());
        }
        _ => {}
    }
}
