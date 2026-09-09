use super::*;

/// A `get` is validly keyed if some equality-constrained column is unique, or the
/// equality columns together cover the model's full composite `@key` (the PK is unique).
pub(super) fn get_is_keyed(q: &Query, ti: usize, cx: &Cx) -> bool {
    let m = cx.model(ti);
    if let QueryBody::Block(s) = &q.body {
        let smi = cx.find(&s.model.node).unwrap_or(ti);
        let sm = cx.model(smi);
        let mut cols = Vec::new();
        for c in &s.clauses {
            if let Clause::Where(p) = c {
                collect_eq_cols(p, &mut cols);
            }
        }
        cols.iter().any(|c| sm.is_unique(c)) || covers_composite_key(sm, &cols)
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
        cols.iter().any(|c| m.is_unique(c)) || covers_composite_key(m, &cols)
    }
}

/// The equality-constrained columns cover the model's full composite `@key` — the PK is a
/// unique index, so keying on every part identifies at most one row.
fn covers_composite_key(m: &RModel, eq_cols: &[String]) -> bool {
    m.is_composite_key() && m.key.iter().all(|k| eq_cols.iter().any(|c| c == k))
}

/// Collect single-segment columns constrained by equality anywhere in a predicate.
fn collect_eq_cols(p: &Predicate, out: &mut Vec<String>) {
    match p {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            collect_eq_cols(a, out);
            collect_eq_cols(b, out);
        }
        Predicate::Not(inner) => collect_eq_cols(inner, out),
        Predicate::Cmp {
            path, op: Op::Eq, ..
        } if path.segments.len() == 1 => {
            out.push(path.segments[0].node.clone());
        }
        _ => {}
    }
}
