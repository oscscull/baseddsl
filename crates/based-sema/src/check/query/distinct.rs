use super::*;

/// `list distinct` guards. `distinct` dedups the projected rows (`SELECT DISTINCT`), so:
/// a keyset `page` is rejected (its hidden id/cursor columns are part of the row and
/// defeat the dedup); an aggregate query is redundant (a `group by` already returns
/// distinct groups); each explicit `order` column must be projected (Postgres rejects
/// `SELECT DISTINCT … ORDER BY <unselected>`, enforced uniformly); and it warns when the
/// projection already carries the primary key (every row is then unique).
pub(super) fn check_distinct(q: &Query, ret: &Resolved, ti: usize, agg: bool, cx: &Cx, sink: &mut Sink) {
    let stmt = match &q.body {
        QueryBody::Block(s) if s.distinct => s,
        _ => return,
    };
    if agg {
        reject_distinct_aggregate(q, sink);
        return;
    }
    reject_distinct_keyset(q, stmt, sink);

    // The shape's top-level scalar projections. `None` = a bare-model return (projects
    // every column, primary key included).
    let projected: Option<Vec<&[Ident]>> = ret
        .shape
        .as_deref()
        .and_then(|n| cx.shape_bodies.get(n).copied())
        .map(projected_paths);

    match &projected {
        Some(paths) => {
            check_distinct_order_projected(q, stmt, paths, sink);
            warn_distinct_pk_noop(q, ti, paths, cx, sink);
        }
        None => warn_distinct_whole_row(q, ret, sink),
    }
}

/// `distinct` on an aggregate query is redundant — a `group by` already returns distinct
/// groups.
fn reject_distinct_aggregate(q: &Query, sink: &mut Sink) {
    sink.error_note(
        code::DISTINCT_AGGREGATE,
        q.span,
        format!(
            "`distinct` on aggregate query `{}` is redundant",
            q.name.node
        ),
        "a `group by` already returns one row per distinct group — drop `distinct`",
    );
}

/// A keyset `page` needs the unique id column in the row, which defeats `distinct`.
fn reject_distinct_keyset(q: &Query, stmt: &Statement, sink: &mut Sink) {
    if stmt
        .clauses
        .iter()
        .any(|c| matches!(c, Clause::Page(p) if !p.offset))
    {
        sink.error_note(
            code::DISTINCT_KEYSET,
            q.span,
            format!("`distinct` query `{}` uses a keyset `page`", q.name.node),
            "a keyset cursor needs the unique id column, which defeats `distinct` — use `page (…) offset`",
        );
    }
}

/// Under `distinct` every `order` column must be projected (Postgres rejects an unselected
/// `ORDER BY` column).
fn check_distinct_order_projected(q: &Query, stmt: &Statement, paths: &[&[Ident]], sink: &mut Sink) {
    for c in &stmt.clauses {
        if let Clause::Order(terms) = c {
            for t in terms {
                if !paths.iter().any(|p| same_segments(p, &t.path.segments)) {
                    sink.error_note(
                        code::DISTINCT_ORDER_UNPROJECTED,
                        q.span,
                        format!(
                            "`distinct` query `{}` orders by unprojected column `{}`",
                            q.name.node,
                            join_path(&t.path)
                        ),
                        "under `distinct` every `order` column must be projected (Postgres rejects an unselected `ORDER BY` column) — project it or drop it from `order`",
                    );
                }
            }
        }
    }
}

/// A shape projecting the primary key already yields unique rows, so `distinct` is redundant.
fn warn_distinct_pk_noop(q: &Query, ti: usize, paths: &[&[Ident]], cx: &Cx, sink: &mut Sink) {
    let pks = cx.model(ti).pk_field_names();
    if paths
        .iter()
        .any(|p| p.len() == 1 && pks.contains(&p[0].node.as_str()))
    {
        sink.warn_note(
            code::DISTINCT_NOOP,
            q.span,
            format!(
                "`distinct` query `{}` projects the primary key",
                q.name.node
            ),
            "a projected primary key makes every row unique — `distinct` has no effect",
        );
    }
}

/// A full-model return already includes the primary key, so every row is unique — `distinct`
/// is redundant.
fn warn_distinct_whole_row(q: &Query, ret: &Resolved, sink: &mut Sink) {
    sink.warn_note(
        code::DISTINCT_NOOP,
        q.span,
        format!(
            "`distinct` query `{}` returns whole `{}` rows",
            q.name.node, ret.model
        ),
        "a full-model row already includes the primary key, so every row is unique — `distinct` has no effect",
    );
}

/// The top-level scalar column paths a shape projects (as segment slices): bare fields
/// and `out = path` renames. Nests, flattens, raw expressions, and aggregates are not
/// plain orderable columns, so they don't count toward the `distinct` order rule.
fn projected_paths(body: &[ShapeField]) -> Vec<&[Ident]> {
    let mut out = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => out.push(std::slice::from_ref(id)),
            ShapeField::Rename {
                value: ShapeValue::Path(p),
                ..
            } => out.push(p.segments.as_slice()),
            _ => {}
        }
    }
    out
}

/// Two segment slices naming the same path (segment-for-segment). The slice twin of
/// [`same_path`], used to match an `order` term against a shape's projected columns.
fn same_segments(a: &[Ident], b: &[Ident]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.node == y.node)
}
