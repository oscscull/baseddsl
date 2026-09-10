use super::*;

/// The per-dialect ceiling on bound parameters in one statement, above which a bulk INSERT
/// is chunked (BW1). Postgres's wire protocol caps at 65535; SQLite's compile-time variable
/// limit is smaller and version-dependent, so a conservative value keeps every build safe.
/// The user never sees the cap — the engine chunks transparently.
pub(crate) fn max_binds(dialect: based_codegen::Dialect) -> usize {
    match dialect {
        based_codegen::Dialect::Sqlite => 900,
        _ => 65000,
    }
}

/// Execute a structured shape-input create and its nested-write children (BW1 + nested
/// writes). Order: create **to-one** children first (their key fills this insert's FK), run
/// this insert, then create **to-many** children (this insert's key fills their back-FK).
/// `inject` supplies FK values this step's parent computed (a to-many child's back-FK).
/// Returns each written row's primary key (in `pk_recover` order) when `want_pk`, so a
/// parent can link to it. Every insert runs on the same transaction connection, so the whole
/// nested write is atomic with the surrounding mutation.
pub(crate) async fn exec_bulk<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: based_codegen::Dialect,
    step: &crate::plan::BulkStep,
    env: &std::collections::HashMap<String, SqlValue>,
    want_pk: bool,
    inject: &[ColInject],
) -> Result<Vec<Vec<SqlValue>>, DbError> {
    // A fillable copy of the rows — nested-write FK columns are overwritten before insert.
    let mut rows = step.rows.clone();
    // Parent-supplied back-FK values (a to-many child links to its parent's key).
    for ci in inject {
        for (i, v) in ci.per_row.iter().enumerate() {
            if let Some(row) = rows.get_mut(i) {
                if ci.bind < row.len() {
                    row[ci.bind] = v.clone();
                }
            }
        }
    }
    // To-one children first: create each, then splice its key into this insert's FK.
    for nc in &step.nested_one {
        let child_pks = Box::pin(exec_bulk(&mut *db, dialect, &nc.step, env, true, &[])).await?;
        for (j, &parent) in nc.parent_of.iter().enumerate() {
            for slot in &nc.link_slots {
                let Some(idx) = nc
                    .step
                    .pk_recover
                    .iter()
                    .position(|p| p.field == slot.key_field)
                else {
                    continue;
                };
                if let (Some(prow), Some(cval)) = (
                    rows.get_mut(parent),
                    child_pks.get(j).and_then(|pk| pk.get(idx)),
                ) {
                    if slot.bind < prow.len() {
                        prow[slot.bind] = cval.clone();
                    }
                }
            }
        }
    }

    // This insert must expose its key when a caller wants it OR a to-many child links to it.
    let need_pk = want_pk || !step.nested_many.is_empty();
    let want_serial = need_pk && step.serial_return.is_some();
    let serial_ids = insert_bulk_rows(db, dialect, step, &rows, env, want_serial).await?;

    let pk_rows: Vec<Vec<SqlValue>> = if need_pk {
        rows.iter()
            .enumerate()
            .map(|(i, row)| {
                step.pk_recover
                    .iter()
                    .map(|p| match p.bind {
                        Some(b) => row[b].clone(),
                        None => serial_ids.get(i).cloned().unwrap_or(SqlValue::Null),
                    })
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };

    // To-many children after: this insert's key fills each child's back-FK, per child row.
    for nm in &step.nested_many {
        let child_inject: Vec<ColInject> = nm
            .link_slots
            .iter()
            .filter_map(|slot| {
                let idx = step
                    .pk_recover
                    .iter()
                    .position(|p| p.field == slot.key_field)?;
                let per_row = nm
                    .parent_of
                    .iter()
                    .map(|&parent| {
                        pk_rows
                            .get(parent)
                            .and_then(|pk| pk.get(idx))
                            .cloned()
                            .unwrap_or(SqlValue::Null)
                    })
                    .collect();
                Some(ColInject {
                    bind: slot.bind,
                    per_row,
                })
            })
            .collect();
        Box::pin(exec_bulk(
            &mut *db,
            dialect,
            &nm.step,
            env,
            false,
            &child_inject,
        ))
        .await?;
    }

    if want_pk {
        Ok(pk_rows)
    } else {
        Ok(Vec::new())
    }
}

/// Insert already-resolved rows as one or more multi-row `INSERT … VALUES (…),(…),…`
/// statements, transparently chunked below the driver's bind limit. Recovers the
/// DB-generated `serial` id per row when `want_serial` (`RETURNING` on Postgres/SQLite, the
/// `LAST_INSERT_ID()` range on MySQL/MariaDB). Zero rows is a no-op.
pub(crate) async fn insert_bulk_rows<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: based_codegen::Dialect,
    step: &crate::plan::BulkStep,
    rows: &[Vec<SqlValue>],
    env: &std::collections::HashMap<String, SqlValue>,
    want_serial: bool,
) -> Result<Vec<SqlValue>, DbError> {
    use based_codegen::Dialect;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let col_list = step
        .columns
        .iter()
        .map(|c| c.quoted.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let per_row = step.binds_per_row.max(1);
    // A bulk upsert's tail binds (a param / `$ctx`) repeat once per chunk statement, so they
    // count against the chunk's bind budget alongside the per-row binds.
    let tail_binds = step.conflict_tail.as_ref().map_or(0, |t| count_named(t));
    let chunk_rows = ((max_binds(dialect).saturating_sub(tail_binds)) / per_row).max(1);
    // The `serial` id uses `RETURNING` on Postgres/SQLite; MySQL/MariaDB have none, so the
    // ids come from the `LAST_INSERT_ID()` range (the first id + a contiguous block).
    let returning = if want_serial {
        step.serial_return.as_ref()
    } else {
        None
    };
    let use_returning =
        returning.is_some() && matches!(dialect, Dialect::Postgres | Dialect::Sqlite);
    let mut serial_ids: Vec<SqlValue> = Vec::new();

    for chunk in rows.chunks(chunk_rows) {
        let mut sql = format!("INSERT INTO {} ({col_list})\nVALUES ", step.table);
        let mut params: Vec<SqlValue> = Vec::with_capacity(chunk.len() * per_row + tail_binds);
        let mut ord = 0usize;
        for (r, row) in chunk.iter().enumerate() {
            if r > 0 {
                sql.push_str(", ");
            }
            sql.push('(');
            let mut bind_i = 0usize;
            for (ci, c) in step.columns.iter().enumerate() {
                if ci > 0 {
                    sql.push_str(", ");
                }
                if let Some(lit) = &c.literal {
                    sql.push_str(lit);
                } else {
                    ord += 1;
                    match dialect {
                        Dialect::Postgres => {
                            sql.push('$');
                            sql.push_str(&ord.to_string());
                        }
                        _ => sql.push('?'),
                    }
                    params.push(row[bind_i].clone());
                    bind_i += 1;
                }
            }
            sql.push(')');
        }
        // Bulk upsert (BW2): the `ON CONFLICT … / ON DUPLICATE KEY UPDATE` tail, its `:name`
        // binds continuing this statement's positional count.
        if let Some(tail) = &step.conflict_tail {
            let (frag, tparams) = crate::scan::to_positional_from(tail, dialect, ord, |n| {
                env.get(n).cloned().map(SqlValue::expand)
            })
            .map_err(|n| DbError::new(format!("unbound placeholder `:{n}` (upsert tail)")))?;
            sql.push_str(&frag);
            params.extend(tparams);
        }
        if use_returning {
            let scol = returning.unwrap();
            sql.push_str(&format!(" RETURNING {}", dialect.quote(scol)));
            sql.push_str(";\n");
            let rows = fetch_all(db.fetch(&sql, &params)).await?;
            for mut row in rows {
                let v = row.remove(scol).unwrap_or(serde_json::Value::Null);
                serial_ids.push(json_to_key(&v));
            }
        } else if returning.is_some() {
            // MySQL/MariaDB: no `INSERT … RETURNING` — the ids are the contiguous
            // `LAST_INSERT_ID()` range (first id .. first + chunk length).
            sql.push_str(";\n");
            db.execute(&sql, &params).await?;
            let first = fetch_all(db.fetch("SELECT LAST_INSERT_ID() AS id", &[]))
                .await?
                .into_iter()
                .next()
                .and_then(|r| r.get("id").and_then(serde_json::Value::as_i64))
                .ok_or_else(|| DbError::new("LAST_INSERT_ID() returned no row"))?;
            for i in 0..chunk.len() as i64 {
                serial_ids.push(SqlValue::Int(first + i));
            }
        } else {
            sql.push_str(";\n");
            db.execute(&sql, &params).await?;
        }
    }
    Ok(serial_ids)
}

/// Count the `:name` placeholders in a SQL fragment (quote-aware via [`to_positional`]) —
/// how many binds a bulk upsert's tail contributes to each chunk statement.
pub(crate) fn count_named(sql: &str) -> usize {
    crate::scan::to_positional(sql, based_codegen::Dialect::Postgres, |_| {
        Some(crate::scan::Bound::One(()))
    })
    .map_or(0, |(_, v)| v.len())
}

/// A read-back key value from a fetched JSON scalar: an integer id stays an `Int`, anything
/// else its text — enough for equality-keying rows back to their input order.
pub(crate) fn json_to_key(v: &serde_json::Value) -> SqlValue {
    match v {
        serde_json::Value::Number(n) if n.is_i64() => SqlValue::Int(n.as_i64().unwrap()),
        serde_json::Value::Number(n) if n.is_u64() => SqlValue::Int(n.as_u64().unwrap() as i64),
        serde_json::Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// The declared-shape read-back for a structured `create … from` (BW1b/BW2): re-select the
/// written rows keyed on their keys (an `IN (…)` over the captured key tuples), reorder them
/// to input order via the hidden `__bkk_<i>` key columns, and return one object (`-> Shape`)
/// or an array (`-> Shape[]`). Reuses the shape projection, so nested shapes decode as on a
/// read. Zero written rows → an empty array (bulk) / not-found (single).
pub(crate) async fn run_bulk_readback<D: DbRead + ?Sized>(
    db: &mut D,
    plan: &MutationPlan,
    rb: &crate::plan::BulkReadbackPlan,
    keys: &[Vec<SqlValue>],
    env: &std::collections::HashMap<String, SqlValue>,
) -> Result<TxOutcome, DbError> {
    use serde_json::Value as J;
    if keys.is_empty() {
        return if rb.bulk {
            Ok(TxOutcome::Done(J::Array(Vec::new())))
        } else {
            Ok(TxOutcome::NotFound)
        };
    }

    // Splice the key-tuple IN-list into the sentinel, as `:__bk_<row>_<col>` binds.
    let mut binds: std::collections::HashMap<String, SqlValue> = std::collections::HashMap::new();
    let mut tuples: Vec<String> = Vec::with_capacity(keys.len());
    for (r, key) in keys.iter().enumerate() {
        let parts: Vec<String> = key
            .iter()
            .enumerate()
            .map(|(c, v)| {
                let name = format!("__bk_{r}_{c}");
                binds.insert(name.clone(), v.clone());
                format!(":{name}")
            })
            .collect();
        tuples.push(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            format!("({})", parts.join(", "))
        });
    }
    let sql = rb.sql.replace(
        based_codegen::sql::mutations::BULK_KEYS_SENTINEL,
        &tuples.join(", "),
    );
    let (bound_sql, params) = crate::scan::to_positional(&sql, plan.dialect, |n| {
        env.get(n)
            .or_else(|| binds.get(n))
            .cloned()
            .map(SqlValue::expand)
    })
    .map_err(|n| DbError::new(format!("unbound placeholder `:{n}` (bulk read-back)")))?;
    let rows = fetch_all(db.fetch(&bound_sql, &params)).await?;

    // Map each fetched row by its hidden key columns (stripped before nesting), then emit in
    // input-key order (a duplicate input key repeats its row; a missing key is skipped).
    let mut by_key: std::collections::HashMap<String, J> = std::collections::HashMap::new();
    let alias_prefix = based_codegen::sql::mutations::BULK_KEY_ALIAS;
    for mut row in rows {
        let mut kparts: Vec<J> = Vec::with_capacity(rb.key_count);
        for c in 0..rb.key_count {
            kparts.push(row.remove(&format!("{alias_prefix}{c}")).unwrap_or(J::Null));
        }
        let mut v = nest_row(row);
        normalize_json(&mut v, &plan.json_paths);
        by_key.insert(norm_key(&kparts), v);
    }
    let mut out: Vec<J> = Vec::with_capacity(keys.len());
    for key in keys {
        let kparts: Vec<J> = key.iter().map(sql_value_to_json).collect();
        if let Some(v) = by_key.get(&norm_key(&kparts)) {
            out.push(v.clone());
        }
    }

    if rb.bulk {
        Ok(TxOutcome::Done(J::Array(out)))
    } else {
        match out.into_iter().next() {
            Some(v) => Ok(TxOutcome::Done(v)),
            None => Ok(TxOutcome::NotFound),
        }
    }
}

/// A canonical string key for a tuple of JSON scalars — equal iff the values are equal — so a
/// fetched row's hidden key columns match the captured input key regardless of source.
pub(crate) fn norm_key(parts: &[serde_json::Value]) -> String {
    parts
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// The `{ id }` fallback response value for a created row's id — a uuid/ulid string or a
/// DB-generated integer, mirroring its wire type.
pub(crate) fn sql_value_to_json(v: &SqlValue) -> serde_json::Value {
    match v {
        SqlValue::Int(i) => serde_json::Value::Number((*i).into()),
        SqlValue::Uuid(s) | SqlValue::Text(s) => serde_json::Value::String(s.clone()),
        other => serde_json::json!(format!("{other:?}")),
    }
}

/// A per-row override for one FK column of a nested-write step: the column's bind index and
/// its value in each row (the parent's key, for a to-many child).
pub(crate) struct ColInject {
    bind: usize,
    per_row: Vec<SqlValue>,
}
