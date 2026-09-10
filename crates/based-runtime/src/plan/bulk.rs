use super::*;

/// The coercion family for a captured column: the type of the created model's `field`
/// member (a scalar's primitive, a relation terminal's target key). Defaults to `Any`
/// when unresolved — a plain text bind.
pub(crate) fn capture_family(schema: &CheckedSchema, model: &str, field: &str) -> Family {
    schema
        .model(model)
        .and_then(|m| member_family(schema, m, &[field]))
        .unwrap_or(Family::Any)
}

/// Resolve a structured shape-input create (BW1) for execution: read the row(s) from the
/// shape param, then, per row, mint app ids, inject the `$ctx` scope, and coerce each
/// payload value by its column's family — so the run stage only has to chunk + bind.
pub(crate) fn build_bulk_step(
    compiled: &Compiled,
    req: &Request,
    id_gen: &dyn IdGen,
    env: &Env,
    bulk: &based_codegen::sql::mutations::BulkInsert,
) -> Result<BulkStep, PlanError> {
    // The top-level row(s): an array for `create Model[] from`, a single object for `create
    // Model from`. A missing / wrong-shaped arg is a boundary error (never SQL).
    let rows_json = read_bulk_rows(req, bulk)?;
    build_bulk_step_rows(compiled, id_gen, env, bulk, &rows_json)
}

/// Materialize a [`BulkStep`] from a model's already-read input rows — the shared core of
/// the top-level create and every nested-write child. Recurses into `bulk.nested_one`,
/// extracting each child's payload from `rows_json[i][relation]`.
pub(crate) fn build_bulk_step_rows(
    compiled: &Compiled,
    id_gen: &dyn IdGen,
    env: &Env,
    bulk: &based_codegen::sql::mutations::BulkInsert,
    rows_json: &[&serde_json::Map<String, serde_json::Value>],
) -> Result<BulkStep, PlanError> {
    use based_codegen::sql::mutations::BulkSource;
    use serde_json::Value as J;
    let schema = &compiled.schema;
    let model = schema
        .model(&bulk.model)
        .ok_or_else(|| PlanError::UnboundPlaceholder(format!("bulk model `{}`", bulk.model)))?;

    let columns: Vec<BulkOutCol> = bulk
        .columns
        .iter()
        .map(|c| BulkOutCol {
            quoted: compiled.dialect.quote(&c.column),
            literal: matches!(c.source, BulkSource::Now).then(|| "CURRENT_TIMESTAMP".to_string()),
        })
        .collect();
    let binds_per_row = bulk
        .columns
        .iter()
        .filter(|c| !matches!(c.source, BulkSource::Now))
        .count();

    let mut rows: Vec<Vec<SqlValue>> = Vec::with_capacity(rows_json.len());
    for obj in rows_json {
        let mut vals = Vec::with_capacity(binds_per_row);
        for c in &bulk.columns {
            match &c.source {
                BulkSource::Now => {}
                BulkSource::MintUuid => vals.push(SqlValue::Uuid(id_gen.next_id())),
                BulkSource::MintUlid => vals.push(SqlValue::Uuid(id_gen.next_ulid())),
                // A nested-write FK (to-one child's key, or a to-many child's parent key) is
                // filled at run time — a placeholder now, overwritten before this insert runs.
                BulkSource::NestedOneId { .. } | BulkSource::ParentId { .. } => {
                    vals.push(SqlValue::Null);
                }
                BulkSource::Ctx { ctx_field } => {
                    vals.push(
                        env.values
                            .get(&format!("ctx_{ctx_field}"))
                            .cloned()
                            .unwrap_or(SqlValue::Null),
                    );
                }
                BulkSource::Field { json_key, field } => {
                    let fam = member_family(schema, model, &[field]).unwrap_or(Family::Any);
                    let jv = obj.get(json_key).cloned().unwrap_or(J::Null);
                    vals.push(
                        coerce(&jv, fam, true)
                            .map_err(|e| bad_arg(&format!("{}.{json_key}", bulk.param), e))?,
                    );
                }
                BulkSource::FkPart {
                    relation,
                    key_field,
                } => {
                    let fam = member_family(schema, model, &[relation]).unwrap_or(Family::Uuid);
                    let jv = obj
                        .get(relation)
                        .and_then(|r| r.get(key_field))
                        .cloned()
                        .unwrap_or(J::Null);
                    vals.push(coerce(&jv, fam, true).map_err(|e| {
                        bad_arg(&format!("{}.{relation}.{key_field}", bulk.param), e)
                    })?);
                }
            }
        }
        rows.push(vals);
    }

    let nested_one = build_nested_one(compiled, id_gen, env, bulk, rows_json)?;
    let nested_many = build_nested_many(compiled, id_gen, env, bulk, rows_json)?;
    let key_rows = bulk_key_rows(bulk, &rows);
    // A DB-generated `serial` primary-key part is recovered from the INSERT (`RETURNING` /
    // `LAST_INSERT_ID()`), whether for the top-level read-back or a nested-write child link.
    let serial_return = bulk
        .pk_parts
        .iter()
        .find(|p| p.serial)
        .map(|p| p.column.clone())
        .or_else(|| {
            bulk.readback_serial
                .then(|| bulk.returning.first().cloned())
                .flatten()
        });
    let pk_recover = bulk_pk_recover(bulk);

    Ok(BulkStep {
        table: bulk.table.clone(),
        columns,
        rows,
        binds_per_row,
        conflict_tail: bulk.conflict_tail.clone(),
        key_rows,
        serial_return,
        nested_one,
        nested_many,
        pk_recover,
    })
}

/// The per-row bind index of a physical column among a bulk insert's non-literal columns
/// (skipping engine literals like `CURRENT_TIMESTAMP`). `None` if the column is not bound.
pub(crate) fn bulk_bind_index(
    bulk: &based_codegen::sql::mutations::BulkInsert,
    column: &str,
) -> Option<usize> {
    use based_codegen::sql::mutations::BulkSource;
    bulk.columns
        .iter()
        .filter(|c| !matches!(c.source, BulkSource::Now))
        .position(|c| c.column == column)
}

/// How to recover each primary-key part per row (for a parent to link its FK): a `serial`
/// part comes from the INSERT, every other part from a bound column.
pub(crate) fn bulk_pk_recover(bulk: &based_codegen::sql::mutations::BulkInsert) -> Vec<PkRecover> {
    bulk.pk_parts
        .iter()
        .map(|p| PkRecover {
            field: p.field.clone(),
            bind: if p.serial {
                None
            } else {
                bulk_bind_index(bulk, &p.column)
            },
        })
        .collect()
}

/// Build the to-one nested-write children of a bulk insert: for each `nested_one` relation,
/// extract each parent row's child object, plan the child insert, and record the parent
/// linkage (which parent each child row fills, and which FK columns take the child's key).
pub(crate) fn build_nested_one(
    compiled: &Compiled,
    id_gen: &dyn IdGen,
    env: &Env,
    bulk: &based_codegen::sql::mutations::BulkInsert,
    rows_json: &[&serde_json::Map<String, serde_json::Value>],
) -> Result<Vec<NestedBulk>, PlanError> {
    use based_codegen::sql::mutations::BulkSource;
    let mut out = Vec::with_capacity(bulk.nested_one.len());
    for nc in &bulk.nested_one {
        // The parent FK columns this child fills, and which of its key fields feeds each.
        let link_slots: Vec<LinkSlot> = bulk
            .columns
            .iter()
            .filter(|c| !matches!(c.source, BulkSource::Now))
            .enumerate()
            .filter_map(|(bind, c)| match &c.source {
                BulkSource::NestedOneId { nest, key_field } if *nest == nc.relation => {
                    Some(LinkSlot {
                        bind,
                        key_field: key_field.clone(),
                    })
                }
                _ => None,
            })
            .collect();

        // Each parent row's child payload (a to-one object). A missing / null object leaves
        // the parent's FK unfilled (an optional relation); a non-object is a boundary error.
        let mut child_rows: Vec<&serde_json::Map<String, serde_json::Value>> = Vec::new();
        let mut parent_of: Vec<usize> = Vec::new();
        for (i, obj) in rows_json.iter().enumerate() {
            match obj.get(&nc.relation) {
                Some(serde_json::Value::Object(o)) => {
                    child_rows.push(o);
                    parent_of.push(i);
                }
                None | Some(serde_json::Value::Null) => {}
                Some(_) => {
                    return Err(bad_arg(
                        &format!("{}.{}", bulk.param, nc.relation),
                        CoerceError {
                            expected: Family::Any,
                            got: "a non-object nested-write value".to_string(),
                        },
                    ))
                }
            }
        }
        let step = build_bulk_step_rows(compiled, id_gen, env, &nc.child, &child_rows)?;
        out.push(NestedBulk {
            step,
            parent_of,
            link_slots,
        });
    }
    Ok(out)
}

/// Build the to-many nested-write children of a bulk insert: for each `nested_many`
/// relation, flatten every parent row's child array (tracking each child's parent), plan the
/// child insert, and record the child's back-FK columns (filled from the parent's key).
pub(crate) fn build_nested_many(
    compiled: &Compiled,
    id_gen: &dyn IdGen,
    env: &Env,
    bulk: &based_codegen::sql::mutations::BulkInsert,
    rows_json: &[&serde_json::Map<String, serde_json::Value>],
) -> Result<Vec<NestedBulk>, PlanError> {
    use based_codegen::sql::mutations::BulkSource;
    let mut out = Vec::with_capacity(bulk.nested_many.len());
    for nc in &bulk.nested_many {
        // The child's back-FK columns (bind index into the *child* rows) and which parent key
        // field feeds each.
        let link_slots: Vec<LinkSlot> = nc
            .child
            .columns
            .iter()
            .filter(|c| !matches!(c.source, BulkSource::Now))
            .enumerate()
            .filter_map(|(bind, c)| match &c.source {
                BulkSource::ParentId { key_field } => Some(LinkSlot {
                    bind,
                    key_field: key_field.clone(),
                }),
                _ => None,
            })
            .collect();

        // Flatten every parent's child array; a missing / null array is an empty collection.
        let mut child_rows: Vec<&serde_json::Map<String, serde_json::Value>> = Vec::new();
        let mut parent_of: Vec<usize> = Vec::new();
        for (i, obj) in rows_json.iter().enumerate() {
            match obj.get(&nc.relation) {
                Some(serde_json::Value::Array(a)) => {
                    for v in a {
                        let o = v.as_object().ok_or_else(|| {
                            bad_arg(
                                &format!("{}.{}", bulk.param, nc.relation),
                                CoerceError {
                                    expected: Family::Any,
                                    got: "a non-object to-many element".to_string(),
                                },
                            )
                        })?;
                        child_rows.push(o);
                        parent_of.push(i);
                    }
                }
                None | Some(serde_json::Value::Null) => {}
                Some(_) => {
                    return Err(bad_arg(
                        &format!("{}.{}", bulk.param, nc.relation),
                        CoerceError {
                            expected: Family::Any,
                            got: "a non-array to-many nested-write value".to_string(),
                        },
                    ))
                }
            }
        }
        let step = build_bulk_step_rows(compiled, id_gen, env, &nc.child, &child_rows)?;
        out.push(NestedBulk {
            step,
            parent_of,
            link_slots,
        });
    }
    Ok(out)
}

/// Read a structured create's row(s) from its shape param: an array for the bulk form, a
/// single object for the single form. A missing / wrong-shaped arg is a boundary error.
pub(crate) fn read_bulk_rows<'a>(
    req: &'a Request,
    bulk: &based_codegen::sql::mutations::BulkInsert,
) -> Result<Vec<&'a serde_json::Map<String, serde_json::Value>>, PlanError> {
    use serde_json::Value as J;
    match req.args.get(&bulk.param) {
        Some(J::Array(a)) if bulk.bulk => a
            .iter()
            .map(|v| {
                v.as_object().ok_or_else(|| PlanError::BadArg {
                    name: bulk.param.clone(),
                    expected: Family::Any,
                    got: "a non-object array element".to_string(),
                })
            })
            .collect(),
        Some(J::Object(o)) if !bulk.bulk => Ok(vec![o]),
        None => Err(PlanError::MissingArg(bulk.param.clone())),
        Some(other) => Err(PlanError::BadArg {
            name: bulk.param.clone(),
            expected: Family::Any,
            got: format!(
                "{} (expected {})",
                json_kind(other),
                if bulk.bulk {
                    "an array of objects"
                } else {
                    "an object"
                }
            ),
        }),
    }
}

/// The app-known read-back key of each written row (BW1b/BW2): the bind index of each
/// read-back key column among the per-row bound values, then that value per row, in input
/// order. Empty for a `serial` (DB-generated) key or a `-> ok` insert (no read-back).
pub(crate) fn bulk_key_rows(
    bulk: &based_codegen::sql::mutations::BulkInsert,
    rows: &[Vec<SqlValue>],
) -> Vec<Vec<SqlValue>> {
    use based_codegen::sql::mutations::BulkSource;
    if bulk.readback_serial || bulk.readback_key.is_empty() {
        return Vec::new();
    }
    let bind_idx: Option<Vec<usize>> = bulk
        .readback_key
        .iter()
        .map(|kc| {
            bulk.columns
                .iter()
                .filter(|c| !matches!(c.source, BulkSource::Now))
                .position(|c| &c.column == kc)
        })
        .collect();
    match bind_idx {
        Some(idx) => rows
            .iter()
            .map(|r| idx.iter().map(|&i| r[i].clone()).collect())
            .collect(),
        None => Vec::new(),
    }
}
