//! Whole-query raw body: what stays legal through the SQL hatch, and what is rejected.

use super::*;

mod params;
mod soft_delete_gap;

use params::*;
use soft_delete_gap::*;

/// Check a whole-query raw body. The engine keeps only param-binding and shape-typed
/// results through this hatch, so anything beyond that is rejected here: params must be
/// typed bind values (a raw body has no column to infer a type from, and no engine-built
/// WHERE for a binding to ride), `$ctx` has no type source, `scoped` would promise an
/// injection the engine cannot make, and streaming and nested shapes rely on engine-built
/// SQL. Soft-delete is the one gap that stays legal — linted, so it stays visible.
pub(super) fn check_raw_query(
    q: &Query,
    raw: &RawSql,
    ti: usize,
    cx: &Cx,
    params: &[String],
    sink: &mut Sink,
) {
    check_raw_query_params(q, raw, params, sink);
    if q.ret.stream {
        sink.error_note(
            code::RAW_QUERY_STREAM,
            q.ret.ty.span,
            format!("raw-bodied query `{}` can't `stream`", q.name.node),
            "collect with `-> Shape[]`, or write an engine-built `list` body",
        );
    }
    if let Some(s) = &q.scoped {
        sink.error_note(
            code::RAW_QUERY_SCOPED,
            s.span,
            format!(
                "`scoped` on raw-bodied query `{}` — the engine can't inject a scope predicate into raw SQL",
                q.name.node
            ),
            "write the scope filter in the SQL yourself and mark the query `unscoped(\"…\")`",
        );
    }
    // The declared shape types the result columns by name; a nested sub-object
    // depends on engine-built projections (join aliases / JSON aggregation) that a
    // raw statement does not get.
    if let Some(body) = cx.shape_bodies.get(&q.ret.ty.node) {
        if body.iter().any(|f| {
            matches!(
                f,
                ShapeField::Nest { .. } | ShapeField::NestRef { .. } | ShapeField::Flatten { .. }
            )
        }) {
            sink.error_note(
                code::RAW_QUERY_NEST,
                q.ret.ty.span,
                format!(
                    "raw-bodied query `{}` returns shape `{}`, which nests a sub-object",
                    q.name.node, q.ret.ty.node
                ),
                "a raw body can't build nested projections — return a flat shape",
            );
        }
    }
    check_raw_soft_delete_gap(raw, ti, cx, sink);
}
