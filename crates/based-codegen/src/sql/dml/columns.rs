//! Physical SQL columns for a model's fields and primary key.

use super::*;

/// A model's physical primary-key column for a JOIN / correlation `ON` — the `id`
/// column, or a `@key(field)` natural key. Falls back to `id` for a model with no
/// resolved key (a keyless model can't be a relation endpoint — sema flags that — so the
/// fallback only ever fires on an already-erroring schema, keeping the SQL well-formed).
pub(crate) fn pk_col(m: &RModel) -> String {
    m.pk_column().unwrap_or_else(|| "id".to_string())
}

/// Physical column backing a scalar field (its `(column …)` override or its name).
pub(crate) fn column_of(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        _ => field.to_string(),
    }
}

/// Physical column backing any field: a scalar's column, or a forward relation's FK
/// (`<field>_id`). Falls back to the field name (inverse edge / unknown — the latter
/// sema already rejected). Used by the write side to map `field = $x` assignments.
pub(crate) fn physical_col(model: &RModel, field: &str) -> String {
    match model.member(field).map(|m| &m.kind) {
        Some(MemberKind::Scalar { column, .. }) => column.clone(),
        Some(MemberKind::Forward { fk_col, .. }) => fk_col.clone(),
        _ => field.to_string(),
    }
}
