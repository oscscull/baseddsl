use super::*;

/// Lower a `delete` / `hard delete`. `where_` is `None` for the whole-table wipe `delete
/// all` — every row in scope. A soft model's plain `delete[ all]` becomes a tombstone
/// UPDATE; `hard delete[ all]` and a plain model emit a real DELETE.
pub(crate) fn lower_delete(
    cx: &LowerCx,
    model: &RModel,
    where_: Option<&Predicate>,
    hard: bool,
) -> LoweredWrite {
    let wipe = where_.is_none();
    if let (Some(sd), false) = (&model.soft_delete, hard) {
        return soft_delete_write(cx, model, sd, where_, wipe);
    }
    real_delete_write(cx, model, where_, hard, wipe)
}

/// A soft model's plain `delete[ all]`: a tombstone UPDATE over the live rows in scope.
fn soft_delete_write(
    cx: &LowerCx,
    model: &RModel,
    sd: &SoftDelete,
    where_: Option<&Predicate>,
    wipe: bool,
) -> LoweredWrite {
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
    let mut sets = vec![tombstone_set(&sel, model, sd, /* deleting = */ true)];
    if let Some(bump) = updated_bump(&sel, model, &[]) {
        sets.push(bump);
    }
    // `delete all` narrows by nothing user-supplied; only the injected live + scope guards
    // remain (tombstone every live row in scope).
    let mut wheres: Vec<String> = where_.map(|p| sel.predicate(p, model)).into_iter().collect();
    inject_guards(&mut sel, model, &mut wheres, /* live = */ true);
    let header = if wipe {
        "-- delete all (soft): tombstone every row in scope\n"
    } else {
        "-- delete (soft): tombstone the matched rows\n"
    };
    LoweredWrite {
        header: header.to_string(),
        sql: update_stmt(&sel, model, &sets, &wheres),
        model: model.name.clone(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: false,
        capture: None,
        wipe,
        bulk: None,
        real_delete: false,
    }
}

/// A plain model, or the loud `hard delete` opt-out: a real DELETE. A `[hard ]delete all`
/// with no remaining WHERE (unscoped / non-scoped) is a whole-table wipe — `TRUNCATE` where
/// transaction-safe (Postgres), else `DELETE FROM t`; a scoped wipe keeps its scope
/// predicate, so it stays a `DELETE FROM t WHERE <scope>`.
fn real_delete_write(
    cx: &LowerCx,
    model: &RModel,
    where_: Option<&Predicate>,
    hard: bool,
    wipe: bool,
) -> LoweredWrite {
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
    let mut wheres: Vec<String> = where_.map(|p| sel.predicate(p, model)).into_iter().collect();
    inject_guards(&mut sel, model, &mut wheres, /* live = */ false);

    let sql = if wipe && wheres.is_empty() {
        cx.dialect.wipe_all(&sel.qt(model))
    } else {
        delete_stmt(&sel, model, &wheres)
    };
    let header = match (hard, wipe) {
        (true, true) => "-- hard delete all: whole-table wipe\n",
        (true, false) => "-- hard delete: real DELETE (explicit soft-delete opt-out)\n",
        (false, true) => "-- delete all: whole-table wipe\n",
        (false, false) => "",
    };
    LoweredWrite {
        header: header.to_string(),
        sql,
        model: model.name.clone(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: false,
        capture: None,
        wipe,
        bulk: None,
        // A filtered real DELETE ack-checks its zero-rows-affected as a 404; a wipe does not.
        real_delete: !wipe,
    }
}
