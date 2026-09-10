use super::*;

pub(crate) fn lower_restore(cx: &LowerCx, model: &RModel, where_: &Predicate) -> LoweredWrite {
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
    // sema (E-restore) guarantees a soft-delete model here; fall back defensively.
    let mut sets = match &model.soft_delete {
        Some(sd) => vec![tombstone_set(&sel, model, sd, /* deleting = */ false)],
        None => Vec::new(),
    };
    if let Some(bump) = updated_bump(&sel, model, &[]) {
        sets.push(bump);
    }
    // Restore targets the *deleted* rows, so the live predicate is NOT injected;
    // `@scope` still applies (you can only restore within your scope) unless `unscoped`.
    let mut wheres = vec![sel.predicate(where_, model)];
    if let Some(scope) = sel.scope_where(&sel.root_alias, model) {
        wheres.push(scope);
    }
    LoweredWrite {
        header: "-- restore: clear the tombstone\n".to_string(),
        sql: update_stmt(&sel, model, &sets, &wheres),
        model: model.name.clone(),
        gen_id: None,
        conflict_key: None,
        read_key: None,
        serial_col: None,
        creates: false,
        capture: None,
        wipe: false,
        bulk: None,
        real_delete: false,
    }
}
