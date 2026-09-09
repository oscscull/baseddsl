use super::*;

/// Validate `where`/`order`/`page` clauses; returns whether an `order` is present.
pub(super) fn check_clauses(
    clauses: &[Clause],
    mi: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) -> bool {
    let mut has_order = false;
    for c in clauses {
        match c {
            Clause::Where(p) => resolve::check_predicate(p, Some(mi), cx, params, sink),
            Clause::Order(terms) => {
                has_order = true;
                for t in terms {
                    resolve::check_sort_term(t, mi, cx, sink);
                }
            }
            Clause::Page(_) => {}
            // Validated by the index pass: it either satisfies the index lint or is itself
            // flagged stale when the query turns out indexed.
            Clause::Unindexed(_) => {}
            // Aggregation clauses: legal only on an aggregate query, where
            // `check_agg_query` validates them; `reject_agg_clauses` reports them here.
            Clause::GroupBy(_) | Clause::Having(_) => {}
        }
    }
    has_order
}
