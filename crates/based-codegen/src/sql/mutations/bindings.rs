use std::collections::HashMap;

use super::*;

/// Which columns each `create … as name` binding's siblings read (`$name.field`), so the
/// bound create's row read-back projects exactly those. Walks the flattened body: first
/// maps each binding to its model, then collects every `$name.field` reference (an assign
/// RHS, an arithmetic operand, a composite-FK whole-row `$name`, or a `where` comparison)
/// to a captured column. Mirrors `Select::binding_field_value`'s resolution so the
/// captured columns match the emitted `:bref_<name>__<column>` binds.
pub(crate) fn collect_binding_refs<'a>(
    schema: &'a CheckedSchema,
    body: &'a [WriteStmt],
) -> HashMap<String, Vec<CaptureCol>> {
    let flat = flat_writes(body);
    let mut models: HashMap<&str, &RModel> = HashMap::new();
    for w in &flat {
        if let WriteStmt::Create {
            model,
            binding: Some(b),
            ..
        } = w
        {
            if let Some(m) = schema.model(&model.node) {
                models.insert(b.node.as_str(), m);
            }
        }
    }
    let mut out: HashMap<String, Vec<CaptureCol>> = HashMap::new();
    for w in &flat {
        match w {
            WriteStmt::Create {
                model,
                assigns,
                conflict,
                ..
            } => {
                let m = schema.model(&model.node);
                collect_assign_refs(schema, m, assigns, &models, &mut out);
                if let Some(oc) = conflict {
                    collect_assign_refs(schema, m, &oc.update, &models, &mut out);
                }
            }
            WriteStmt::Update {
                model,
                where_,
                assigns,
            } => {
                collect_assign_refs(
                    schema,
                    schema.model(&model.node),
                    assigns,
                    &models,
                    &mut out,
                );
                collect_pred_refs(where_, &models, &mut out);
            }
            WriteStmt::Restore { where_, .. } => collect_pred_refs(where_, &models, &mut out),
            WriteStmt::Delete { where_, .. } | WriteStmt::HardDelete { where_, .. } => {
                if let Some(p) = where_ {
                    collect_pred_refs(p, &models, &mut out);
                }
            }
            WriteStmt::Tx(_) | WriteStmt::Raw(_) => {}
        }
    }
    out
}

/// Record `$binding.field` as a captured column of the bound model: its physical column,
/// deduped, under the `bref_<binding>__<column>` bind a `$binding.field` reference reads.
fn add_ref(
    out: &mut HashMap<String, Vec<CaptureCol>>,
    models: &HashMap<&str, &RModel>,
    binding: &str,
    field: &str,
) {
    let Some(m) = models.get(binding) else { return };
    let column = physical_col(m, field);
    let cols = out.entry(binding.to_string()).or_default();
    if !cols.iter().any(|c| c.column == column) {
        cols.push(CaptureCol {
            bind: bref_name(binding, &column),
            column,
            field: field.to_string(),
        });
    }
}

/// Collect binding references from a write's assign block.
fn collect_assign_refs(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    assigns: &[Assign],
    models: &HashMap<&str, &RModel>,
    out: &mut HashMap<String, Vec<CaptureCol>>,
) {
    for a in assigns {
        collect_rhs_refs(schema, model, &a.col.node, &a.value, models, out);
    }
}

/// Collect binding references from one assign RHS. A whole-row `$name` filling a
/// composite-FK column references every key part of the bound model; a `$name.field`
/// references that one field; an arithmetic RHS recurses into its operands.
fn collect_rhs_refs(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    col: &str,
    rhs: &AssignRhs,
    models: &HashMap<&str, &RModel>,
    out: &mut HashMap<String, Vec<CaptureCol>>,
) {
    match rhs {
        AssignRhs::Value(Value::Param(pr)) if models.contains_key(pr.name.node.as_str()) => {
            let binding = pr.name.node.as_str();
            if let Some(field) = pr.path.first() {
                add_ref(out, models, binding, &field.node);
            } else if let Some(mem) = model.and_then(|m| m.member(col)) {
                // Whole-row `$name` into a composite-FK column: capture every key part.
                if matches!(mem.kind, MemberKind::Forward { .. }) {
                    let parts = schema.fk_columns(mem);
                    if parts.len() > 1 {
                        for (_, part) in parts {
                            add_ref(out, models, binding, &part.name);
                        }
                    }
                }
            }
        }
        AssignRhs::Value(_) => {}
        AssignRhs::Arith { lhs, rhs, .. } => {
            collect_rhs_refs(schema, model, col, lhs, models, out);
            collect_rhs_refs(schema, model, col, rhs, models, out);
        }
    }
}

/// Collect binding references from a `where` predicate: a `$name.field` on either side of a
/// comparison, in an `in (…)` list, or passed as a named-filter argument.
fn collect_pred_refs(
    pred: &Predicate,
    models: &HashMap<&str, &RModel>,
    out: &mut HashMap<String, Vec<CaptureCol>>,
) {
    let record = |out: &mut HashMap<String, Vec<CaptureCol>>, v: &Value| {
        if let Value::Param(pr) = v {
            if let Some(field) = pr.path.first() {
                add_ref(out, models, &pr.name.node, &field.node);
            }
        }
    };
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            collect_pred_refs(a, models, out);
            collect_pred_refs(b, models, out);
        }
        Predicate::Not(inner) => collect_pred_refs(inner, models, out),
        Predicate::Cmp { value, .. } => record(out, value),
        Predicate::InList { values, .. } => values.iter().for_each(|v| record(out, v)),
        Predicate::FilterCall { args, .. } => args.iter().for_each(|v| record(out, v)),
        Predicate::Bare(_) | Predicate::Raw(_) => {}
    }
}
