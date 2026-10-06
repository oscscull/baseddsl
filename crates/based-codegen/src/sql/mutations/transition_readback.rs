//! Read a conditional transition back by its unchanged, explicitly bound primary key.

use super::*;

pub(crate) struct TransitionReadback {
    pub key: Predicate,
    pub write_index: usize,
}

pub(crate) fn transition_readback(
    m: &Mutation,
    rm: Option<&RMutation>,
    schema: &CheckedSchema,
    stmts: &[LoweredWrite],
) -> Option<TransitionReadback> {
    let rm = rm.filter(|rm| !rm.ack)?;
    if stmts
        .iter()
        .any(|stmt| stmt.creates && stmt.model == rm.ret_model)
    {
        return None;
    }
    // A preceding create/return write owns the result; do not change its read-back.
    let target = flat_writes(&m.body).into_iter().find(|stmt| match stmt {
        WriteStmt::Create { model, .. }
        | WriteStmt::Update { model, .. }
        | WriteStmt::Delete { model, .. }
        | WriteStmt::HardDelete { model, .. }
        | WriteStmt::Restore { model, .. } => model.node == rm.ret_model,
        WriteStmt::Tx(_) | WriteStmt::Raw(_) => false,
    })?;
    let WriteStmt::Update {
        model,
        where_,
        assigns,
    } = target
    else {
        return None;
    };
    if !reads_assigned_field(where_, assigns) {
        return None;
    }
    let model = schema.model(&model.node)?;
    let keys = model.pk_members();
    if keys.is_empty()
        || keys
            .iter()
            .any(|key| assigns.iter().any(|a| a.col.node == key.name))
    {
        return None;
    }
    let key = keys
        .into_iter()
        .map(|key| bound_key(where_, &key.name).cloned())
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .reduce(|a, b| Predicate::And(Box::new(a), Box::new(b)))?;
    let write_index = stmts.iter().position(|stmt| stmt.model == rm.ret_model)?;
    Some(TransitionReadback { key, write_index })
}

/// Only equalities required by conjunction can uniquely identify the written row.
/// A key inside OR/NOT, or a value read from a changing column, is not a stable key.
fn bound_key<'a>(pred: &'a Predicate, field: &str) -> Option<&'a Predicate> {
    match pred {
        Predicate::And(a, b) => bound_key(a, field).or_else(|| bound_key(b, field)),
        Predicate::Cmp {
            path,
            op: Op::Eq,
            value: Value::Param(_) | Value::Lit(_),
        } if local_field(path) == Some(field) => Some(pred),
        _ => None,
    }
}

fn local_field(path: &Path) -> Option<&str> {
    match path.segments.as_slice() {
        [field] => Some(field.node.as_str()),
        _ => None,
    }
}

fn reads_assigned_field(pred: &Predicate, assigns: &[Assign]) -> bool {
    let assigned = |path: &Path| {
        local_field(path).is_some_and(|field| assigns.iter().any(|a| a.col.node == field))
    };
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            reads_assigned_field(a, assigns) || reads_assigned_field(b, assigns)
        }
        Predicate::Not(inner) => reads_assigned_field(inner, assigns),
        Predicate::Cmp { path, value, .. } => {
            assigned(path) || matches!(value, Value::Path(path) if assigned(path))
        }
        Predicate::InList { path, .. } | Predicate::Bare(path) => assigned(path),
        Predicate::FilterCall { .. } | Predicate::Raw(_) => false,
    }
}
