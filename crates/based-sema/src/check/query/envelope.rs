use super::*;

/// The result-envelope rules, judged once the body is known: a scalar `get` must be
/// keyed, a `list` must sort deterministically, `stream` and `page` are exclusive, and a
/// keyset page over a keyless model needs a unique sort key.
pub(super) fn check_query_envelope(
    q: &Query,
    ti: usize,
    shape: &QueryShape,
    cx: &Cx,
    sink: &mut Sink,
) {
    let engine_built = !shape.raw && !shape.agg;
    check_stream_get_cardinality(q, shape, sink);
    check_query_cardinality(q, shape, sink);
    check_get_keyed(q, ti, shape, engine_built, cx, sink);
    check_stream_page_exclusive(q, shape, sink);
    check_nondet_sort(q, ti, shape, engine_built, cx, sink);
    check_keyset_keyless(q, ti, engine_built, cx, sink);
}

/// An explicit body verb must agree with the declared return envelope. Bare and inline
/// queries infer their verb from that envelope, but a block can otherwise pair `list` with a
/// scalar client type (or `get` with a collection) and defer the mismatch to wire decoding.
fn check_query_cardinality(q: &Query, shape: &QueryShape, sink: &mut Sink) {
    if q.ret.stream || q.ret.many == (shape.verb == Verb::List) {
        return;
    }

    let (message, note) = if q.ret.many {
        (
            format!(
                "list query `{}` uses `get` — the declared return is many rows",
                q.name.node
            ),
            "use `list`, or drop `[]` for a scalar return",
        )
    } else {
        (
            format!(
                "scalar query `{}` uses `list` — the declared return is one row",
                q.name.node
            ),
            "use `get`, or add `[]` for a list return",
        )
    };
    sink.error_note(code::QUERY_CARDINALITY, q.span, message, note);
}

/// A stream is a list delivered incrementally; a `get` body is a cardinality mismatch.
fn check_stream_get_cardinality(q: &Query, shape: &QueryShape, sink: &mut Sink) {
    if q.ret.stream && shape.verb == Verb::Get {
        sink.error_note(
            code::STREAM_GET,
            q.ret.ty.span,
            format!(
                "stream query `{}` uses `get` — a stream is many rows",
                q.name.node
            ),
            "use `list`, or drop `stream` for a scalar return",
        );
    }
}

/// A scalar `get` must be keyed on a unique field.
fn check_get_keyed(
    q: &Query,
    ti: usize,
    shape: &QueryShape,
    engine_built: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    if shape.verb == Verb::Get && !q.ret.stream && engine_built && !get_is_keyed(q, ti, cx) {
        sink.error_note(
            code::GET_NOT_UNIQUE,
            q.span,
            format!(
                "`get` query `{}` is not keyed on a unique field",
                q.name.node
            ),
            "a scalar `get` needs an equality on `id`, a `(unique)` column, or a unique index",
        );
    }
}

/// A page is a bounded chunk + a re-entry cursor; a stream is one unbounded forward pass —
/// the envelopes contradict.
fn check_stream_page_exclusive(q: &Query, shape: &QueryShape, sink: &mut Sink) {
    if q.ret.stream && shape.paginated {
        sink.error_note(
            code::STREAM_PAGE,
            q.span,
            format!("stream query `{}` declares `page`", q.name.node),
            "paginate for random access, stream for the full pass — drop one",
        );
    }
}

/// Nondeterministic-order lint: a `list` with no sort at any tier.
fn check_nondet_sort(
    q: &Query,
    ti: usize,
    shape: &QueryShape,
    engine_built: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    if shape.verb == Verb::List && engine_built && !shape.has_order && cx.model(ti).sort.is_empty()
    {
        sink.warn(
            code::NONDET_SORT,
            q.span,
            format!(
                "`list` query `{}` has no sort — results are nondeterministic; add `order (…)` or a model `@sort`",
                q.name.node
            ),
        );
    }
}

/// A keyless model has no `id` tiebreaker, so a keyset page (non-offset `page`) needs a sort
/// that is itself a total order — its effective sort must include a local `(unique)` column,
/// else the minted cursor could drop or repeat rows.
fn check_keyset_keyless(q: &Query, ti: usize, engine_built: bool, cx: &Cx, sink: &mut Sink) {
    let keyset = page_clause(&q.body).is_some_and(|p| !p.offset);
    let m = cx.model(ti);
    if m.no_id && engine_built && keyset {
        let deterministic = effective_sort(&q.body, m)
            .iter()
            .any(|t| t.path.segments.len() == 1 && m.is_unique(&t.path.segments[0].node));
        if !deterministic {
            sink.error_note(
                code::KEYLESS_KEYSET,
                q.span,
                format!("keyset `page` on keyless `{}` has no unique sort key", m.name),
                "a `@no_id` model has no `id` tiebreaker — `order (…)` on a `(unique)` column, or `page (…) offset`",
            );
        }
    }
}

/// The query's `page` clause, if any (inline or block body).
fn page_clause(body: &QueryBody) -> Option<&PageClause> {
    let clauses: &[Clause] = match body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        _ => return None,
    };
    clauses.iter().find_map(|c| match c {
        Clause::Page(p) => Some(p),
        _ => None,
    })
}

/// The query's effective sort terms: its `order` clause, else the model's `@sort`.
fn effective_sort<'a>(body: &'a QueryBody, model: &'a RModel) -> &'a [SortTerm] {
    let clauses: &[Clause] = match body {
        QueryBody::Inline(cs) => cs,
        QueryBody::Block(s) => &s.clauses,
        _ => return &model.sort,
    };
    clauses
        .iter()
        .find_map(|c| match c {
            Clause::Order(t) => Some(t.as_slice()),
            _ => None,
        })
        .unwrap_or(&model.sort)
}
