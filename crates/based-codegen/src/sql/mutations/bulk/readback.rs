use super::super::*;

/// The physical columns a structured `create … from`'s declared-shape read-back keys on,
/// and whether they are DB-generated (`serial`, learned from the INSERT). Mirrors the
/// singular create's key resolution: an upsert reads back on the conflict target; a plain
/// create on the surrogate id (`serial` → learned; uuid/ulid → app-minted), the natural /
/// composite `@key`, or (keyless) a `(unique)` column the shape set.
pub(super) fn bulk_readback_key(
    cx: &LowerCx,
    model: &RModel,
    conflict: Option<&OnConflict>,
    serial_col: Option<&str>,
    columns: &[BulkCol],
) -> (Vec<String>, bool) {
    if let Some(oc) = conflict {
        // The conflict target's value keys the read-back (a conflict path keeps the existing
        // row, so a generated id would miss it) — app-known from the payload.
        return (
            oc.target
                .iter()
                .map(|t| physical_col(model, &t.node))
                .collect(),
            false,
        );
    }
    // A sole DB-generated `serial` id: learned from the INSERT (not in the payload).
    if model.pk_is_db_generated() {
        return (vec![physical_col(model, "id")], true);
    }
    // A surrogate app-minted id (uuid/ulid).
    if !model.no_id && model.key.is_empty() {
        return (vec![physical_col(model, "id")], false);
    }
    // A natural / composite `@key`: its columns are app-supplied. (A composite key with a
    // `serial` part in a from-create read-back is unsupported — an exotic combination.)
    if !model.key.is_empty() && serial_col.is_none() {
        return (
            model.key.iter().map(|f| physical_col(model, f)).collect(),
            false,
        );
    }
    // A keyless (`@no_id`) model: the first `(unique)` column the shape set.
    let _ = cx;
    let present: Vec<&str> = columns.iter().map(|c| c.column.as_str()).collect();
    for u in &model.unique_cols {
        let col = physical_col(model, u);
        if present.contains(&col.as_str()) {
            return (vec![col], false);
        }
    }
    (Vec::new(), false)
}
