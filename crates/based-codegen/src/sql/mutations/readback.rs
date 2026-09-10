use super::*;

/// How a declared-shape re-select keys the row it reads back.
enum RetKey<'a> {
    /// The mutation *created* the row — key on its engine id (`WHERE id = :result_id`,
    /// bound by the runtime). The row is live.
    CreatedId,
    /// The mutation *updated / soft-deleted / restored* the row — key on that write's
    /// own `where` predicate (its params/`$ctx` are already bound). `live` selects whether
    /// the soft-delete live predicate rides along: true for update/restore (the row is
    /// live), false for a soft delete (the row is tombstoned but must still read back).
    Where { pred: &'a Predicate, live: bool },
    /// The mutation *upserted* the row (`create … on conflict`) — key on the conflict
    /// target's inserted value, since a conflict path keeps the existing row's id (so the
    /// INSERT's generated id misses it). Each pair is `(physical_col, value_sql)`. The
    /// row is live (upsert is disallowed on a soft-delete model).
    Conflict(Vec<(String, String)>),
    /// The mutation created a composite-`@key` row with a DB-generated `serial` part — key
    /// on the captured serial value (`serial_col = :result_id`, bound late by the runtime)
    /// and the other app-supplied key parts (`others`, each `(physical_col, value_sql)`).
    CompositeSerial {
        serial_col: String,
        others: Vec<(String, String)>,
    },
}

/// Select the declared-shape re-select for a mutation's written row, whenever that row
/// survives the write, keyed the way the runtime's `plan_mutation` keys it: on `:result_id`
/// when a write creates the return row, else the write's own `where` for a surviving update
/// / soft delete / restore. A real DELETE removes the row → no re-select → `{}` at runtime.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ret_select(
    schema: &CheckedSchema,
    decls: &[Decl],
    m: &Mutation,
    rm: Option<&RMutation>,
    stmts: &[LoweredWrite],
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
) -> Option<String> {
    rm.and_then(|rm| {
        // An upsert (`create … on conflict`) on the return model keys on the conflict target
        // (a conflict path keeps the existing row's id, so the generated id won't match); a
        // plain create keys on that id; an update / soft delete / restore keys on its `where`.
        let upsert = stmts
            .iter()
            .find(|w| w.conflict_key.is_some() && w.model == rm.ret_model);
        // A composite `@key` create with a DB-generated `serial` part keys the re-select on
        // the captured serial value (`:result_id`) and its other app-supplied key parts.
        let composite_serial = stmts.iter().find(|w| {
            w.serial_col.is_some() && w.read_key.is_some() && w.model == rm.ret_model
        });
        // A keyless create reads back by the `(unique)` column it set — the same
        // `WHERE col = value` shape as a conflict key.
        let keyless = stmts.iter().find(|w| {
            w.read_key.is_some() && w.serial_col.is_none() && w.model == rm.ret_model
        });
        // A create of the return row keys the re-select on that row's id — app-minted
        // (`gen_id`) or DB-generated (a sole `serial` id, bound late from the captured id).
        // Both use `WHERE id = :result_id`.
        let creates_ret = stmts
            .iter()
            .any(|w| (w.gen_id.is_some() || w.serial_col.is_some()) && w.model == rm.ret_model);
        let key = if let Some(w) = upsert {
            RetKey::Conflict(w.conflict_key.clone().unwrap_or_default())
        } else if let Some(w) = composite_serial {
            RetKey::CompositeSerial {
                serial_col: w.serial_col.clone().unwrap_or_default(),
                others: w.read_key.clone().unwrap_or_default(),
            }
        } else if let Some(w) = keyless {
            RetKey::Conflict(w.read_key.clone().unwrap_or_default())
        } else if creates_ret {
            RetKey::CreatedId
        } else {
            let (pred, live) = surviving_ret_write(&m.body, &rm.ret_model, schema)?;
            RetKey::Where { pred, live }
        };
        Some(lower_ret_select(
            schema,
            decls,
            &rm.ret_model,
            rm.ret_shape.as_deref(),
            unscoped,
            inject,
            dialect,
            key,
        ))
    })
}

/// Select the IN-keyed read-back for a structured `create … from` of the return model that
/// returns a shape (not `-> ok`): it reads its written rows back over their keys, in place
/// of the where-/create-keyed [`ret_select`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn bulk_readback(
    schema: &CheckedSchema,
    decls: &[Decl],
    rm: Option<&RMutation>,
    stmts: &[LoweredWrite],
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
) -> Option<BulkReadback> {
    rm.and_then(|rm| {
        if rm.ack {
            return None;
        }
        let w = stmts
            .iter()
            .find(|w| w.model == rm.ret_model && w.bulk.is_some())?;
        let bi = w.bulk.as_ref()?;
        if bi.readback_key.is_empty() {
            return None;
        }
        Some(lower_bulk_readback(
            schema,
            decls,
            &rm.ret_model,
            rm.ret_shape.as_deref(),
            unscoped,
            inject,
            dialect,
            bi,
        ))
    })
}

/// Build the declared-shape re-select for a mutation's written row: the same projection a
/// `get` of that shape emits (`project_return`, reused from the read side so the two stay in
/// lockstep), keyed per `key` (created-id or write-`where`). The soft-delete live predicate
/// (when the row is live) and `@scope` ride the read path exactly as a `get` would, so a row
/// that lands / lives out of scope reads back as absent, consistent with every other read.
#[allow(clippy::too_many_arguments)]
fn lower_ret_select(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret_model: &str,
    ret_shape: Option<&str>,
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
    key: RetKey,
) -> String {
    let model = schema
        .model(ret_model)
        .expect("return model resolved by sema");
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);

    // Projection first (it seeds joins for reached columns), then the row key + guards
    // (which may seed more joins — a relation-reaching write `where`).
    let projection = project_return(&mut sel, decls, ret_shape, ret_model, model);
    let (mut wheres, live) = match key {
        RetKey::CreatedId => (
            vec![format!("{} = :result_id", sel.qcol(&sel.root_alias, "id"))],
            true,
        ),
        RetKey::Where { pred, live } => (vec![sel.predicate(pred, model)], live),
        RetKey::Conflict(pairs) => (
            pairs
                .iter()
                .map(|(c, v)| format!("{} = {v}", sel.qcol(&sel.root_alias, c)))
                .collect(),
            true,
        ),
        RetKey::CompositeSerial { serial_col, others } => {
            let mut wheres = vec![format!(
                "{} = :result_id",
                sel.qcol(&sel.root_alias, &serial_col)
            )];
            wheres.extend(
                others
                    .iter()
                    .map(|(c, v)| format!("{} = {v}", sel.qcol(&sel.root_alias, c))),
            );
            (wheres, true)
        }
    };
    if live {
        if let Some(sd) = &model.soft_delete {
            wheres.push(soft_pred(dialect, &sel.root_alias, model, sd));
        }
    }
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }

    let mut sql = format!("SELECT\n{}\nFROM {}", projection, sel.qt(model));
    push_joins(&mut sql, dialect, &sel.joins);
    push_where(&mut sql, &wheres);
    sql.push_str(";\n");
    sql
}

/// Build the declared-shape read-back for a structured `create … from`: the return shape's
/// projection (reusing `project_return`, so nested shapes decode as on a read) plus hidden
/// `__bkk_<i>` key columns, keyed on the written rows' keys via an `IN (/*BULK_KEYS*/)`
/// sentinel the runtime splices with the per-row key binds. The soft-delete live predicate
/// and `@scope` ride the read exactly as a `get` would.
#[allow(clippy::too_many_arguments)]
fn lower_bulk_readback(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret_model: &str,
    ret_shape: Option<&str>,
    unscoped: bool,
    inject: &[ScopeInject],
    dialect: Dialect,
    bi: &BulkInsert,
) -> BulkReadback {
    let model = schema
        .model(ret_model)
        .expect("return model resolved by sema");
    let mut sel = Select::new(schema, decls, model, dialect)
        .with_scope_inject(!unscoped)
        .with_scope_terms(inject);

    let mut projection = project_return(&mut sel, decls, ret_shape, ret_model, model);
    // Hidden key columns so the runtime can pair each fetched row with its input-order key.
    for (i, col) in bi.readback_key.iter().enumerate() {
        projection.push_str(&format!(
            ",\n  {} AS {}",
            sel.qcol(&sel.root_alias, col),
            dialect.quote(&format!("{BULK_KEY_ALIAS}{i}"))
        ));
    }

    let key_tuple: Vec<String> = bi
        .readback_key
        .iter()
        .map(|c| sel.qcol(&sel.root_alias, c))
        .collect();
    let lhs = if key_tuple.len() == 1 {
        key_tuple[0].clone()
    } else {
        format!("({})", key_tuple.join(", "))
    };
    let mut wheres = vec![format!("{lhs} IN ({BULK_KEYS_SENTINEL})")];
    if let Some(sd) = &model.soft_delete {
        wheres.push(soft_pred(dialect, &sel.root_alias, model, sd));
    }
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }

    let mut sql = format!("SELECT\n{}\nFROM {}", projection, sel.qt(model));
    push_joins(&mut sql, dialect, &sel.joins);
    push_where(&mut sql, &wheres);
    sql.push_str(";\n");

    BulkReadback {
        sql,
        key_cols: bi.readback_key.clone(),
        bulk: bi.bulk,
        serial: bi.readback_serial,
    }
}

/// The write whose surviving row a where-keyed re-select reads back: the first
/// `update` / soft `delete` / `restore` on the return model, with its `where` predicate and
/// whether the row is *live* afterwards (so the re-select injects the soft-delete live
/// predicate). A plain-model / `hard delete` removes the row, and a `create` is the
/// create-keyed path — both yield `None` here.
fn surviving_ret_write<'a>(
    body: &'a [WriteStmt],
    ret_model: &str,
    schema: &CheckedSchema,
) -> Option<(&'a Predicate, bool)> {
    for w in flat_writes(body) {
        match w {
            WriteStmt::Update { model, where_, .. } if model.node == ret_model => {
                return Some((where_, true)); // the updated row stays live
            }
            WriteStmt::Restore { model, where_ } if model.node == ret_model => {
                return Some((where_, true)); // the row is live again after a restore
            }
            // A soft `delete` tombstones (the row survives — read it back *without* the live
            // predicate); a plain-model `delete` removes it (skip — no row). A soft `delete
            // all` (`where_` = `None`) tombstones every row, so there is no single surviving
            // row to read back (an `-> ok` ack — skip here too).
            WriteStmt::Delete {
                model,
                where_: Some(where_),
            } if model.node == ret_model
                && schema
                    .model(&model.node)
                    .is_some_and(|m| m.soft_delete.is_some()) =>
            {
                return Some((where_, false));
            }
            _ => {}
        }
    }
    None
}
