//! Input object schemas: one property per signature param (typed from its annotation
//! or inferred from the column it maps to), plus the page-control property.

use super::*;

/// The input object schema for a callable: one property per signature param, typed
/// from its explicit annotation or inferred from the column it maps to. A param with a
/// `(default)` or an optional annotation is not `required` (the engine applies the
/// default). `$ctx` is server context (header), never an input.
pub(crate) fn input_schema(schema: &CheckedSchema, c: &Callable) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for p in c.params {
        let optional =
            p.optional || p.default.is_some() || p.ty.as_ref().is_some_and(|t| t.optional);
        let entity = c.param_entities.get(&p.name.node).map(String::as_str);
        // A `?` optional filter is 2-state (skip / value): not `required`, but not nullable —
        // null-matching is a body concern, not a param state.
        let ty = param_schema(schema, c.root, p, entity);
        props.insert(p.name.node.clone(), ty);
        if !optional {
            required.push(Value::String(p.name.node.clone()));
        }
    }
    // Page control: a keyset page carries the opaque cursor back, an
    // offset page an explicit offset. Both optional (absent = the first page).
    match c.page {
        PageInput::Keyset => {
            props.insert(
                "cursor".to_string(),
                json!({ "type": ["string", "null"], "description": "Opaque keyset cursor from a prior page's `cursor`; omit for the first page." }),
            );
        }
        PageInput::Offset => {
            props.insert(
                "offset".to_string(),
                json!({ "type": "integer", "minimum": 0, "description": "Row offset; omit for the first page." }),
            );
        }
        PageInput::None => {}
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("object"));
    if !required.is_empty() {
        obj.insert("required".to_string(), Value::Array(required));
    }
    obj.insert("properties".to_string(), Value::Object(props));
    Value::Object(obj)
}

/// A param's JSON-Schema type. Explicit annotation wins — an enum name is that
/// enum's constrained schema, a model type the `uuid` FK string the wire carries —
/// otherwise infer from the bound/same-named column. To-many -> an array.
fn param_schema(
    schema: &CheckedSchema,
    root: Option<&RModel>,
    p: &Param,
    entity: Option<&str>,
) -> Value {
    // A param that identifies a model carries that model's primary key on the wire — resolve
    // it (serial → integer, uuid/ulid → string, composite → object), never a blanket uuid.
    // Enum/shape annotations identify no entity and fall through to their own schemas below.
    if let Some(e) = entity {
        let many = p.ty.as_ref().is_some_and(|t| t.many);
        return wrap(fk_target_schema(schema, e), many);
    }
    match &p.ty {
        Some(te) => {
            if let BaseType::Model(name) = &te.base {
                if schema.enum_(&name.node).is_some() {
                    return wrap(enum_schema(schema, &name.node), te.many);
                }
                // A shape-typed param — the row input of a `create … from $p`:
                // `$ref` the shape's component (an array of it for the bulk form).
                if schema.shapes.iter().any(|s| s.name == name.node) {
                    return wrap(schema_ref(&name.node), te.many);
                }
            }
            wrap(base_schema(&te.base), te.many)
        }
        None => infer_param(schema, root, p),
    }
}

/// Infer an untyped param's schema from how it filters (client emitter's twin): an
/// `-> edge` / same-name relation param is the FK (`uuid`); an `op col` binding / same-
/// name scalar takes that column's schema.
fn infer_param(schema: &CheckedSchema, root: Option<&RModel>, p: &Param) -> Value {
    let field = match &p.binding {
        Some(ParamBinding::Edge(edge)) => &edge.node,
        Some(ParamBinding::ColOp { col, .. }) => &col.node,
        None => &p.name.node,
    };
    reach_schema(schema, root, &[field]).0
}
