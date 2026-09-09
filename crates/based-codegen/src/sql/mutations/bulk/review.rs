use super::super::*;

/// A review-only single-row rendering of a bulk insert for `based gen sql` output; the
/// runtime materializes the real, chunked statement from the [`BulkInsert`] plan.
/// Placeholders name each column's per-row source.
pub(super) fn bulk_review_sql(dialect: Dialect, bulk: &BulkInsert) -> String {
    let cols: Vec<String> = bulk
        .columns
        .iter()
        .map(|c| dialect.quote(&c.column))
        .collect();
    let vals: Vec<String> = bulk
        .columns
        .iter()
        .map(|c| match &c.source {
            BulkSource::Field { json_key, .. } => format!(":row.{json_key}"),
            BulkSource::FkPart {
                relation,
                key_field,
            } => format!(":row.{relation}.{key_field}"),
            BulkSource::MintUuid => ":mint(uuid)".to_string(),
            BulkSource::MintUlid => ":mint(ulid)".to_string(),
            BulkSource::Ctx { ctx_field } => format!(":ctx_{ctx_field}"),
            BulkSource::Now => "CURRENT_TIMESTAMP".to_string(),
            BulkSource::NestedOneId { nest, key_field } => format!(":nested({nest}).{key_field}"),
            BulkSource::ParentId { key_field } => format!(":parent.{key_field}"),
        })
        .collect();
    let ret = if bulk.returning.is_empty() {
        String::new()
    } else {
        format!(
            " RETURNING {}",
            bulk.returning
                .iter()
                .map(|c| dialect.quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let tail = bulk.conflict_tail.as_deref().unwrap_or("");
    // Nested-write children are created first (their key feeds this insert's FK). Render
    // them ahead of this INSERT, review-only, so `based gen sql` shows the whole write.
    let mut out = String::new();
    for n in &bulk.nested_one {
        out.push_str(&format!("-- nested write `{}`:\n", n.relation));
        out.push_str(&bulk_review_sql(dialect, &n.child));
    }
    out.push_str(&format!(
        "INSERT INTO {} ({})\nVALUES ({}){tail}{ret};  -- repeated per row, chunked below the driver's bind limit\n",
        bulk.table,
        cols.join(", "),
        vals.join(", "),
    ));
    // To-many children are created after the parent (their back-FK is the parent's key).
    for n in &bulk.nested_many {
        out.push_str(&format!(
            "-- nested write `{}` (to-many, after):\n",
            n.relation
        ));
        out.push_str(&bulk_review_sql(dialect, &n.child));
    }
    out
}
