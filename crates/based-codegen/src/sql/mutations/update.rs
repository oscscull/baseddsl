use std::collections::HashMap;

use super::*;

pub(crate) fn lower_update<'a>(
    cx: &LowerCx<'a>,
    model: &RModel,
    where_: &Predicate,
    assigns: &'a [Assign],
    bindings: &HashMap<&'a str, BackCtx<'a>>,
) -> LoweredWrite {
    let mut sel = Select::new(cx.schema, cx.decls, model, cx.dialect)
        .with_bindings(bindings.clone())
        .with_scope_inject(!cx.unscoped)
        .with_scope_terms(cx.inject);
    let mut sets: Vec<String> = Vec::new();
    let mut assigned: Vec<String> = Vec::new();

    for a in assigns {
        let col = physical_col(model, &a.col.node);
        let val = sel.assign_rhs(&a.value, model, &a.col.node);
        sets.push(format!("{} = {val}", set_lhs(&sel, model, &col)));
        assigned.push(col);
    }
    if let Some(bump) = updated_bump(&sel, model, &assigned) {
        sets.push(bump);
    }

    let mut wheres = vec![sel.predicate(where_, model)];
    inject_guards(&mut sel, model, &mut wheres, /* live = */ true);
    LoweredWrite {
        header: String::new(),
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
