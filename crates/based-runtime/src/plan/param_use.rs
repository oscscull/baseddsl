use super::*;

/// Find the family of the first column `$name` fills or filters across the write body.
pub(crate) fn param_use_in_stmts(
    compiled: &Compiled,
    stmts: &[WriteStmt],
    name: &str,
) -> Option<Family> {
    let schema = &compiled.schema;
    for stmt in stmts {
        let found = match stmt {
            WriteStmt::Create {
                model,
                assigns,
                from: _,
                conflict,
                binding: _,
            } => param_use_in_assigns(schema, &model.node, assigns, name).or_else(|| {
                conflict
                    .as_ref()
                    .and_then(|oc| param_use_in_assigns(schema, &model.node, &oc.update, name))
            }),
            WriteStmt::Update {
                model,
                where_,
                assigns,
            } => param_use_in_assigns(schema, &model.node, assigns, name)
                .or_else(|| param_use_in_pred(compiled, schema.model(&model.node)?, where_, name)),
            WriteStmt::Restore { model, where_ } => {
                param_use_in_pred(compiled, schema.model(&model.node)?, where_, name)
            }
            // `delete all` (`where_` = `None`) binds no params.
            WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => where_
                .as_ref()
                .and_then(|p| param_use_in_pred(compiled, schema.model(&model.node)?, p, name)),
            WriteStmt::Tx(inner) => param_use_in_stmts(compiled, inner, name),
            // A raw write is opaque SQL — its params stay text binds (the escape hatch
            // writes its own casts).
            WriteStmt::Raw(_) => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

pub(crate) fn param_use_in_assigns(
    schema: &CheckedSchema,
    model: &str,
    assigns: &[Assign],
    name: &str,
) -> Option<Family> {
    let m = schema.model(model)?;
    for a in assigns {
        // A `$name` used directly, or as an operand of an arithmetic RHS
        // (`total = total + $name`), binds at the target column's family.
        if assign_rhs_uses_param(&a.value, name) {
            return member_family(schema, m, &[&a.col.node]);
        }
    }
    None
}

pub(crate) fn assign_rhs_uses_param(rhs: &AssignRhs, name: &str) -> bool {
    match rhs {
        AssignRhs::Value(Value::Param(pr)) => pr.name.node == name,
        AssignRhs::Value(_) => false,
        AssignRhs::Arith { lhs, rhs, .. } => {
            assign_rhs_uses_param(lhs, name) || assign_rhs_uses_param(rhs, name)
        }
    }
}

/// Find `$name` in a predicate: a `path op $name` comparison types the param by the
/// path's column; a named-filter call recurses into the filter's own predicate with
/// the call's positional argument mapping.
pub(crate) fn param_use_in_pred(
    compiled: &Compiled,
    model: &RModel,
    pred: &Predicate,
    name: &str,
) -> Option<Family> {
    let schema = &compiled.schema;
    match pred {
        Predicate::Or(a, b) | Predicate::And(a, b) => param_use_in_pred(compiled, model, a, name)
            .or_else(|| param_use_in_pred(compiled, model, b, name)),
        Predicate::Not(inner) => param_use_in_pred(compiled, model, inner, name),
        Predicate::Cmp { path, value, .. } => match value {
            Value::Param(pr) if pr.name.node == name => {
                member_family(schema, model, &path_segments(path))
            }
            _ => None,
        },
        // A `$name` listed in `col in (…)` binds one value of the column's family.
        Predicate::InList { path, values } => values
            .iter()
            .any(|v| matches!(v, Value::Param(pr) if pr.name.node == name))
            .then(|| member_family(schema, model, &path_segments(path)))
            .flatten(),
        Predicate::FilterCall { name: fname, args } => {
            let filter = find_filter(&compiled.decls, &fname.node)?;
            // Positional mapping: the i-th call arg that is `$name` types as the
            // filter's i-th declared param, wherever that param lands in the filter.
            for (arg, fp) in args.iter().zip(&filter.params) {
                if let Value::Param(pr) = arg {
                    if pr.name.node == name {
                        if let Some(f) =
                            param_use_in_pred(compiled, model, &filter.pred, &fp.name.node)
                        {
                            return Some(f);
                        }
                    }
                }
            }
            None
        }
        Predicate::Bare(_) | Predicate::Raw(_) => None,
    }
}
