use std::collections::HashMap;

use super::*;

/// The INSERT column plan built from a create's assigns: the quoted `cols` and their value
/// `vals` (aligned), the `assigned` physical columns, the `value_by_field` map (for the
/// conflict key), and the id decision (`gen_id` app-minted bind / `serial_col` DB-generated).
struct InsertCols {
    cols: Vec<String>,
    vals: Vec<String>,
    assigned: Vec<String>,
    value_by_field: HashMap<String, String>,
    gen_id: Option<String>,
    serial_col: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_create<'a>(
    cx: &LowerCx<'a>,
    model: &RModel,
    assigns: &'a [Assign],
    conflict: Option<&'a OnConflict>,
    id_param: &str,
    bindings: &HashMap<&'a str, BackCtx<'a>>,
    refs: &[CaptureCol],
    claims_result: bool,
) -> LoweredWrite {
    let (schema, decls, dialect) = (cx.schema, cx.decls, cx.dialect);
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_bindings(bindings.clone())
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);

    let InsertCols {
        cols,
        vals,
        assigned,
        value_by_field,
        gen_id,
        serial_col,
    } = build_insert_columns(cx, model, assigns, id_param, &mut sel);

    let read_key = create_read_key(
        model,
        serial_col.as_deref(),
        &assigned,
        &vals,
        &value_by_field,
    );

    let (tail, conflict_key) = upsert_tail_and_key(
        schema,
        decls,
        model,
        conflict,
        dialect,
        &value_by_field,
        /* incoming = */ false,
    );

    // Row read-back (bound create / DB-generated return id) + its `RETURNING` columns.
    let (capture, returning) = build_capture(
        model,
        dialect,
        refs,
        claims_result,
        gen_id.as_deref(),
        serial_col.as_deref(),
        read_key.as_deref(),
        conflict_key.as_deref(),
    );

    LoweredWrite {
        header: format!("-- create {}\n", model.name),
        sql: insert_sql(dialect, model, &cols, &vals, &tail, &returning),
        model: model.name.clone(),
        gen_id,
        conflict_key,
        read_key,
        serial_col,
        creates: true,
        capture,
        wipe: false,
        bulk: None,
        real_delete: false,
    }
}

/// Build a create's INSERT columns: the caller's assigns (a relation into a composite-key
/// model fans out to its FK part columns), the engine-injected `@scope` columns, the primary
/// key (app-minted `:id[_step]` bind, DB-generated `serial` omitted, or a `@key`/keyless
/// caller-supplied column), and the `@created`/`@updated` stamps the caller did not set.
fn build_insert_columns(
    cx: &LowerCx,
    model: &RModel,
    assigns: &[Assign],
    id_param: &str,
    sel: &mut Select,
) -> InsertCols {
    let (schema, dialect) = (cx.schema, cx.dialect);
    let mut cols: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();
    // field name → the value SQL the INSERT sets for it, for building the conflict key.
    let mut value_by_field: HashMap<String, String> = HashMap::new();

    for a in assigns {
        // A relation into a composite-key model is a multi-column FK: the one assign
        // (`enrollment = $e`) fills every `<field>_<part>` column, each part's value pulled
        // from the RHS (a tx binding's key-part assign, or a structured-id param's part).
        let fk_cols = model
            .member(&a.col.node)
            .filter(|m| matches!(m.kind, MemberKind::Forward { .. }))
            .map(|m| schema.fk_columns(m))
            .filter(|p| p.len() > 1);
        if let Some(pairs) = fk_cols {
            for (fk_col, part) in &pairs {
                let val = sel.fk_assign_part(&a.value, &part.name);
                cols.push(dialect.quote(fk_col));
                vals.push(val);
                assigned.push(fk_col.clone());
            }
            continue;
        }
        let col = physical_col(model, &a.col.node);
        cols.push(dialect.quote(&col));
        // An enum column takes a bare variant → its wire string literal.
        let val = sel.assign_rhs(&a.value, model, &a.col.node);
        value_by_field.insert(a.col.node.clone(), val.clone());
        vals.push(val);
        assigned.push(col);
    }

    // `@scope` columns are engine-managed on create: auto-set from `:ctx_<field>` for every
    // axis of the alternative this mutation named, so a caller can only create within their
    // own scope. Sema forbids the caller assigning one, so the `assigned` guard is defensive.
    // Empty when `unscoped`.
    for (field, ctx_field) in sel.scope_terms_for(&model.name).to_vec() {
        let col = physical_col(model, &field);
        value_by_field
            .entry(field.clone())
            .or_insert_with(|| format!(":ctx_{ctx_field}"));
        if !assigned.contains(&col) {
            cols.push(dialect.quote(&col));
            vals.push(format!(":ctx_{ctx_field}"));
            assigned.push(col);
        }
    }

    // The primary key. A `serial` PK is DB-generated: the INSERT *omits* the id column and
    // the runtime reads the assigned value back (its `capture`). An app-minted `id`
    // (uuid/ulid, no SQL default) is bound as `:id[_step]` unless the caller set it. A
    // keyless (`@no_id`) model has no `id` column, and a `@key(field)` model's key is a
    // caller-supplied column set like any other assign.
    let serial_col = serial_return_col(model, dialect);
    let gen_id = if model.no_id || serial_col.is_some() || !model.key.is_empty() {
        None
    } else if !assigned.iter().any(|c| c == "id") {
        cols.insert(0, dialect.quote("id"));
        vals.insert(0, format!(":{id_param}"));
        Some(id_param.to_string())
    } else {
        None
    };

    // `@created`/`@updated` are set on insert, unless the caller already did.
    for col in timestamp_cols(model, &[model.created.as_deref(), model.updated.as_deref()]) {
        if !assigned.contains(&col) {
            cols.push(dialect.quote(&col));
            vals.push("CURRENT_TIMESTAMP".to_string());
        }
    }

    InsertCols {
        cols,
        vals,
        assigned,
        value_by_field,
        gen_id,
        serial_col,
    }
}

/// Build a create's row read-back plan and the `RETURNING` column list its INSERT carries.
/// A bound create captures the columns its siblings reference (`refs`); the return model's
/// create additionally captures its DB-generated id under `result_id` (an app-minted id is
/// known at plan time and needs no capture). On MySQL (no `INSERT … RETURNING`) the capture
/// carries a follow-up keyed `SELECT` and the `RETURNING` list is empty.
#[allow(clippy::too_many_arguments)]
fn build_capture(
    model: &RModel,
    dialect: Dialect,
    refs: &[CaptureCol],
    claims_result: bool,
    gen_id: Option<&str>,
    serial_col: Option<&str>,
    read_key: Option<&[(String, String)]>,
    conflict_key: Option<&[(String, String)]>,
) -> (Option<Capture>, Vec<String>) {
    let mut cap_cols: Vec<CaptureCol> = refs.to_vec();
    if claims_result {
        if let Some(sc) = serial_col {
            let field = model
                .serial_key_member()
                .map_or_else(|| "id".to_string(), |m| m.name.clone());
            cap_cols.push(CaptureCol {
                bind: "result_id".to_string(),
                column: sc.to_string(),
                field,
            });
        }
    }
    if cap_cols.is_empty() {
        return (None, Vec::new());
    }
    // The distinct physical columns to read back, in first-seen order — the `RETURNING`
    // list (Postgres/SQLite/MariaDB) or the follow-up `SELECT` projection (MySQL).
    let mut cols: Vec<String> = Vec::new();
    for c in &cap_cols {
        if !cols.contains(&c.column) {
            cols.push(c.column.clone());
        }
    }
    if dialect == Dialect::MySql {
        let followup_select = followup_select_sql(
            dialect,
            model,
            &cols,
            gen_id,
            serial_col,
            read_key,
            conflict_key,
        );
        (
            Some(Capture {
                cols: cap_cols,
                followup_select: Some(followup_select),
            }),
            Vec::new(),
        )
    } else {
        (
            Some(Capture {
                cols: cap_cols,
                followup_select: None,
            }),
            cols,
        )
    }
}

/// The MySQL follow-up keyed `SELECT` that reads a just-inserted row's committed columns
/// back (MySQL has no `INSERT … RETURNING`). Keyed the same way the declared re-select is:
/// a DB-generated `serial` part on `LAST_INSERT_ID()` (plus any app-supplied composite
/// parts), an app-minted surrogate id on its `:id[_step]` bind, or a natural/keyless unique
/// column on its set value. Unbound `:name` SQL — the run stage binds it.
#[allow(clippy::too_many_arguments)]
fn followup_select_sql(
    dialect: Dialect,
    model: &RModel,
    cols: &[String],
    gen_id: Option<&str>,
    serial_col: Option<&str>,
    read_key: Option<&[(String, String)]>,
    conflict_key: Option<&[(String, String)]>,
) -> String {
    let projection = cols
        .iter()
        .map(|c| dialect.quote(c))
        .collect::<Vec<_>>()
        .join(", ");
    let mut wheres: Vec<String> = Vec::new();
    if let Some(sc) = serial_col {
        wheres.push(format!("{} = LAST_INSERT_ID()", dialect.quote(sc)));
        for (c, v) in read_key.unwrap_or(&[]) {
            wheres.push(format!("{} = {v}", dialect.quote(c)));
        }
    } else if let Some(gid) = gen_id {
        wheres.push(format!("{} = :{gid}", dialect.quote("id")));
    } else if let Some(pairs) = read_key.or(conflict_key) {
        for (c, v) in pairs {
            wheres.push(format!("{} = {v}", dialect.quote(c)));
        }
    }
    format!(
        "SELECT {projection} FROM {} WHERE {};\n",
        dialect.quote_table(model.schema.as_deref(), &model.table),
        wheres.join(" AND ")
    )
}

/// A create with no generated id reads its row back by column(s) it set: a keyless
/// (`@no_id`) model's `(unique)` column, a single `@key(field)`'s column, or the full
/// composite `@key(f1, f2, …)` tuple. A composite key with a `serial` part reads back on its
/// *other* (app-supplied) parts — the serial part rides `:result_id` (the captured DB value).
/// Sema guarantees a keyless declared-shape return sets a unique column; a `@key` model's
/// non-serial key columns are required, always set.
fn create_read_key(
    model: &RModel,
    serial_col: Option<&str>,
    assigned: &[String],
    vals: &[String],
    value_by_field: &HashMap<String, String>,
) -> Option<Vec<(String, String)>> {
    if let Some(sc) = serial_col {
        model
            .is_composite_key()
            .then(|| non_serial_key_pairs(model, sc, assigned, vals))
    } else if model.is_composite_key() {
        composite_read_key(model, assigned, vals)
    } else if model.no_id || !model.key.is_empty() {
        model.unique_cols.iter().find_map(|u| {
            value_by_field
                .get(u)
                .map(|v| vec![(physical_col(model, u), v.clone())])
        })
    } else {
        None
    }
}

/// The read-back key for a composite-`@key` create: every key column the create set, paired
/// with the value SQL it set. `None` if any key column went unset (an already-erroring
/// schema) so codegen never emits a half-keyed re-select.
fn composite_read_key(
    model: &RModel,
    assigned: &[String],
    vals: &[String],
) -> Option<Vec<(String, String)>> {
    let key: Vec<(String, String)> = model
        .key
        .iter()
        .filter_map(|f| {
            let col = physical_col(model, f);
            assigned
                .iter()
                .position(|c| c == &col)
                .map(|i| (col.clone(), vals[i].clone()))
        })
        .collect();
    (key.len() == model.key.len()).then_some(key)
}

/// The DB-generated PK column a `create` recovers from its INSERT (the deferred read-back
/// keys on it): the sole `serial` `id`, or a composite `@key`'s `serial` part on
/// Postgres/MariaDB. `None` otherwise — a composite serial part on SQLite is app-supplied
/// (SQLite has no auto-increment for a non-sole-PK column).
pub(crate) fn serial_return_col(model: &RModel, dialect: Dialect) -> Option<String> {
    if model.pk_is_db_generated() {
        return Some(physical_col(model, "id"));
    }
    model.serial_key_column().filter(|_| {
        matches!(
            dialect,
            Dialect::Postgres | Dialect::MariaDb | Dialect::MySql
        )
    })
}

/// A composite key's app-supplied parts (every key column except the DB-generated `serial`
/// one), each paired with the value SQL the create set — the parts that key the deferred
/// re-select alongside the captured serial value (`:result_id`).
fn non_serial_key_pairs(
    model: &RModel,
    serial_col: &str,
    assigned: &[String],
    vals: &[String],
) -> Vec<(String, String)> {
    model
        .key
        .iter()
        .filter_map(|f| {
            let col = physical_col(model, f);
            if col == serial_col {
                return None;
            }
            assigned
                .iter()
                .position(|c| c == &col)
                .map(|i| (col.clone(), vals[i].clone()))
        })
        .collect()
}

/// Assemble the `INSERT` statement text. A bound create (and a `serial` create's id) reads
/// its written row back via `RETURNING <cols>` on Postgres/SQLite/MariaDB (MySQL has no
/// `INSERT … RETURNING` — its read-back is a follow-up keyed `SELECT`, so `returning` is
/// empty there); a create that sets no columns uses the dialect's default-values form.
fn insert_sql(
    dialect: Dialect,
    model: &RModel,
    cols: &[String],
    vals: &[String],
    tail: &str,
    returning: &[String],
) -> String {
    let table = dialect.quote_table(model.schema.as_deref(), &model.table);
    let returning = if returning.is_empty() {
        String::new()
    } else {
        format!(
            " RETURNING {}",
            returning
                .iter()
                .map(|c| dialect.quote(c))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if cols.is_empty() {
        return match dialect {
            Dialect::Postgres | Dialect::Sqlite => {
                format!("INSERT INTO {table} DEFAULT VALUES{returning};\n")
            }
            Dialect::MariaDb | Dialect::MySql => {
                format!("INSERT INTO {table} () VALUES (){returning};\n")
            }
        };
    }
    format!(
        "INSERT INTO {table} ({})\nVALUES ({}){tail}{returning};\n",
        cols.join(", "),
        vals.join(", "),
    )
}
