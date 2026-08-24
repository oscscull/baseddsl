//! Which entity (model) each callable param identifies — the model whose primary key the
//! param carries on the wire.
//!
//! A param identifies a model when its annotation names one (`parent: Parent`), or — for an
//! untyped or bare-`Id` param — its binding / `= $param` comparison targets a Forward FK or
//! the model's own id (`where (parent = $parent)`). The three code generators (client type,
//! OpenAPI schema, runtime coercion family) each map that entity to their own representation
//! of its key, so a `serial`-keyed reference types as an integer everywhere instead of
//! diverging (client) vs. the project-default uuid (OpenAPI + runtime). Params that are plain
//! scalars, enums, or shapes identify no entity and are absent from the map.

use crate::ir::{CheckedSchema, MemberKind, RModel};
use based_ast::{Assign, BaseType, Mutation, Param, ParamBinding, Predicate, Value};
use std::collections::HashMap;

/// Per-param entity map for a query: each param resolved to the model its key belongs to,
/// via an explicit model annotation or its `-> edge` / `op col` / same-name binding.
pub fn query_param_entities(
    schema: &CheckedSchema,
    root: Option<&RModel>,
    params: &[Param],
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for p in params {
        if let Some(entity) = annotation_entity(schema, p) {
            map.insert(p.name.node.clone(), entity);
            continue;
        }
        let field = binding_field(p);
        if let Some(entity) = root.and_then(|m| member_entity(m, field)) {
            map.insert(p.name.node.clone(), entity);
        }
    }
    map
}

/// Per-param entity map for a mutation: an explicit model annotation, else the model a param
/// is assigned to / compared against in the write body (`col = $param` on a Forward FK / id).
pub fn mutation_param_entities(schema: &CheckedSchema, m: &Mutation) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for p in &m.params {
        if let Some(entity) = annotation_entity(schema, p) {
            map.insert(p.name.node.clone(), entity);
        }
    }
    for stmt in &m.body {
        scan_write(schema, stmt, &mut map);
    }
    map
}

/// The entity named by an explicit annotation — a `BaseType::Model` that resolves to a real
/// model (not an enum or a shape param, which are not id-bearing references).
fn annotation_entity(schema: &CheckedSchema, p: &Param) -> Option<String> {
    let te = p.ty.as_ref()?;
    let BaseType::Model(name) = &te.base else {
        return None;
    };
    if schema.enum_(&name.node).is_some() || schema.shapes.iter().any(|s| s.name == name.node) {
        return None;
    }
    schema.model(&name.node).map(|_| name.node.clone())
}

/// The model a member identifies: a Forward FK's target, or the model itself when the field
/// is its own single-column primary key.
fn member_entity(model: &RModel, field: &str) -> Option<String> {
    match model.member(field).map(|m| &m.kind)? {
        MemberKind::Forward { target, .. } => Some(target.clone()),
        MemberKind::Scalar { .. }
            if !model.is_composite_key() && model.pk_field() == Some(field) =>
        {
            Some(model.name.clone())
        }
        _ => None,
    }
}

/// The field a query param binds against: its `-> edge` / `op col` binding, else its name.
fn binding_field(p: &Param) -> &str {
    match &p.binding {
        Some(ParamBinding::Edge(e)) => &e.node,
        Some(ParamBinding::ColOp { col, .. }) => &col.node,
        None => &p.name.node,
    }
}

fn scan_write(schema: &CheckedSchema, stmt: &based_ast::WriteStmt, map: &mut HashMap<String, String>) {
    use based_ast::WriteStmt;
    match stmt {
        WriteStmt::Create {
            model,
            assigns,
            conflict,
            ..
        } => {
            let m = schema.model(&model.node);
            for a in assigns {
                scan_assign(m, a, map);
            }
            if let Some(oc) = conflict {
                for a in &oc.update {
                    scan_assign(m, a, map);
                }
            }
        }
        WriteStmt::Update {
            model,
            where_,
            assigns,
        } => {
            let m = schema.model(&model.node);
            for a in assigns {
                scan_assign(m, a, map);
            }
            scan_pred(m, where_, map);
        }
        WriteStmt::Restore { model, where_ } => scan_pred(schema.model(&model.node), where_, map),
        WriteStmt::Delete { model, where_ } | WriteStmt::HardDelete { model, where_ } => {
            if let Some(p) = where_ {
                scan_pred(schema.model(&model.node), p, map);
            }
        }
        WriteStmt::Tx(stmts) => {
            for s in stmts {
                scan_write(schema, s, map);
            }
        }
        WriteStmt::Raw(_) => {}
    }
}

/// Record `col = $param` when `col` is a Forward FK / id on `model`.
fn scan_assign(model: Option<&RModel>, a: &Assign, map: &mut HashMap<String, String>) {
    if let Some(Value::Param(pr)) = a.value.as_value() {
        if pr.path.is_empty() {
            if let Some(entity) = model.and_then(|m| member_entity(m, &a.col.node)) {
                map.insert(pr.name.node.clone(), entity);
            }
        }
    }
}

/// Record `col = $param` / `col in ($param, …)` comparisons where `col` is an id member.
fn scan_pred(model: Option<&RModel>, pred: &Predicate, map: &mut HashMap<String, String>) {
    match pred {
        Predicate::And(a, b) | Predicate::Or(a, b) => {
            scan_pred(model, a, map);
            scan_pred(model, b, map);
        }
        Predicate::Not(p) => scan_pred(model, p, map),
        Predicate::Cmp {
            path,
            value: Value::Param(pr),
            ..
        } if path.segments.len() == 1 && pr.path.is_empty() => {
            if let Some(entity) = model.and_then(|m| member_entity(m, &path.segments[0].node)) {
                map.insert(pr.name.node.clone(), entity);
            }
        }
        Predicate::InList { path, values } if path.segments.len() == 1 => {
            for v in values {
                if let Value::Param(pr) = v {
                    if pr.path.is_empty() {
                        if let Some(entity) =
                            model.and_then(|m| member_entity(m, &path.segments[0].node))
                        {
                            map.insert(pr.name.node.clone(), entity);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}
