use std::collections::HashSet;

use super::*;

mod columns;
mod nest;
mod readback;
mod review;

use columns::resolve_from_shape;
use nest::build_bulk_insert;
use readback::bulk_readback_key;
use review::bulk_review_sql;

/// Lower a structured shape-input `create` (`create Model from $row` / `create Model[] from
/// $rows`) to a [`BulkInsert`] column plan plus a review-only single-row template. The
/// runtime materializes the chunked, atomic multi-row INSERT from the plan.
pub(crate) fn lower_bulk_create(
    cx: &LowerCx,
    model: &RModel,
    cf: &CreateFrom,
    conflict: Option<&OnConflict>,
) -> Option<LoweredWrite> {
    let (_from, body) = resolve_from_shape(cx, &cf.param.node)?;
    let dialect = cx.dialect;
    let mut bulk = build_bulk_insert(cx, model, body, cf.param.node.clone(), cf.bulk);
    let serial_col = bulk.returning.first().cloned();

    // A bulk upsert: the per-dialect `ON CONFLICT … / ON DUPLICATE KEY UPDATE` tail, with
    // `incoming.<col>` lowered to `excluded`/`VALUES()`. Bound + appended per chunk. A
    // `...incoming` spread expands to the inserted payload columns (minus the conflict
    // target) read from the proposed row.
    bulk.conflict_tail = conflict.map(|oc| {
        let spread_cols: Vec<String> = if oc.spread.is_some() {
            let target: HashSet<String> = oc
                .target
                .iter()
                .map(|t| physical_col(model, &t.node))
                .collect();
            bulk.columns
                .iter()
                .filter(|bc| matches!(bc.source, BulkSource::Field { .. }))
                .map(|bc| bc.column.clone())
                .filter(|c| !target.contains(c))
                .collect()
        } else {
            Vec::new()
        };
        let sets = conflict_update_sets(
            cx.schema,
            cx.decls,
            model,
            oc,
            dialect,
            /* incoming = */ true,
            &spread_cols,
        );
        upsert_tail(dialect, oc, model, &sets)
    });

    // The declared-shape read-back key: the conflict target for an upsert, else the surrogate
    // id / natural / composite key / a `(unique)` column.
    let (readback_key, readback_serial) =
        bulk_readback_key(cx, model, conflict, serial_col.as_deref(), &bulk.columns);
    bulk.readback_key = readback_key;
    bulk.readback_serial = readback_serial;
    Some(LoweredWrite {
        header: format!(
            "-- create {}{} from ${} (chunked multi-row INSERT — materialized by the runtime)\n",
            model.name,
            if cf.bulk { "[]" } else { "" },
            cf.param.node
        ),
        sql: bulk_review_sql(dialect, &bulk),
        model: model.name.clone(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: true,
        capture: None,
        wipe: false,
        bulk: Some(bulk),
        real_delete: false,
    })
}
