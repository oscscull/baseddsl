use super::*;

/// `for update` guards. A `SELECT … FOR UPDATE` locks the base rows a query reads, so it
/// is legal only where that row set is well-defined and single — the query must be
/// plain-projected (a `distinct` projection is deduped), row-level (a `group by` locks no
/// base row), free of a to-many nest (its json-agg subquery has no lockable row), and
/// non-streaming (a lock held across a long-lived stream is a footgun). Enforced uniformly
/// at compile time so a `FOR UPDATE` boundary surfaces on every dialect. Confinement to
/// transaction clients lives in the generated client (the `TxBound` marker trait).
pub(super) fn check_for_update(
    q: &Query,
    ret: &Resolved,
    ti: usize,
    agg: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let stmt = match &q.body {
        QueryBody::Block(s) if s.for_update.is_some() => s,
        _ => return,
    };
    if stmt.distinct {
        sink.error_note(
            code::FOR_UPDATE_DISTINCT,
            q.span,
            format!("`for update` on `distinct` query `{}`", q.name.node),
            "a deduped projection has no single base row to lock — drop `distinct` or `for update`",
        );
    }
    if agg {
        sink.error_note(
            code::FOR_UPDATE_AGGREGATE,
            q.span,
            format!("`for update` on aggregate query `{}`", q.name.node),
            "a `group by` query locks no individual base row — drop `for update`",
        );
    }
    if q.ret.stream {
        sink.error_note(
            code::FOR_UPDATE_STREAM,
            q.span,
            format!("`for update` on stream query `{}`", q.name.node),
            "a row lock held across a long-lived stream is a footgun — drop `for update` or `stream`",
        );
    }
    if let Some(body) = ret
        .shape
        .as_deref()
        .and_then(|n| cx.shape_bodies.get(n).copied())
    {
        if shape_has_to_many_nest(body, ti, cx, &mut Vec::new()) {
            sink.error_note(
                code::FOR_UPDATE_TOMANY,
                q.span,
                format!("`for update` on query `{}` projecting a to-many nest", q.name.node),
                "a to-many nest lowers to a json-agg subquery with no lockable base row — project it flat, or drop `for update`",
            );
        }
    }
}

/// True when a shape body projects a to-many relation nest at any depth — a `field { … }`
/// (or `field -> Shape`) over a to-many inverse edge, or a `out = path { … }` junction
/// flatten. Recurses through to-one nests; `stack` guards named-shape reference cycles.
fn shape_has_to_many_nest<'a>(
    body: &'a [ShapeField],
    mi: usize,
    cx: &'a Cx,
    stack: &mut Vec<&'a str>,
) -> bool {
    body.iter().any(|f| match f {
        // A junction flatten is always over a to-many edge.
        ShapeField::Flatten { .. } => true,
        ShapeField::Nest { field, body } => match nest_relation(cx, mi, &field.node) {
            Some(NestRel::ToMany) => true,
            Some(NestRel::ToOne(ti)) => shape_has_to_many_nest(body, ti, cx, stack),
            None => false,
        },
        ShapeField::NestRef { field, shape } => match nest_relation(cx, mi, &field.node) {
            Some(NestRel::ToMany) => true,
            Some(NestRel::ToOne(ti)) => {
                if stack.contains(&shape.node.as_str()) {
                    return false; // a reference cycle is handled elsewhere; stop here
                }
                stack.push(&shape.node);
                let found = cx
                    .shape_bodies
                    .get(shape.node.as_str())
                    .copied()
                    .is_some_and(|b| shape_has_to_many_nest(b, ti, cx, stack));
                stack.pop();
                found
            }
            None => false,
        },
        _ => false,
    })
}

/// The cardinality of a relation field for the to-many-nest walk: to-one carries the
/// target model index (to recurse into), to-many needs no further walk.
enum NestRel {
    ToOne(usize),
    ToMany,
}

fn nest_relation(cx: &Cx, mi: usize, field: &str) -> Option<NestRel> {
    match cx.model(mi).member(field).map(|m| &m.kind) {
        Some(MemberKind::Forward { target, .. }) => cx.find(target).map(NestRel::ToOne),
        Some(MemberKind::Inverse { target, via }) => {
            let ti = cx.find(target)?;
            // A unique inverse FK is a one-to-one (to-one); else it is a collection.
            if cx.model(ti).is_unique(via) {
                Some(NestRel::ToOne(ti))
            } else {
                Some(NestRel::ToMany)
            }
        }
        _ => None,
    }
}
