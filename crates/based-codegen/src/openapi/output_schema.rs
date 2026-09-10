//! Output object schemas: resolving a return type to its `components.schemas` entry,
//! projecting shape bodies and bare models into `(field, schema, required)` triples,
//! and the relation walks those projections rely on.

use super::*;

/// Register an output object schema plus every named shape it references (its
/// `nested`), deduped by name — a shape shared by two callables (or referenced from
/// two nests) is one `components.schemas` entry.
pub(crate) fn register_out_schema(
    os: &OutSchema,
    schemas: &mut Map<String, Value>,
    seen: &mut Vec<String>,
) {
    if !os.is_result_fallback && !seen.contains(&os.name) {
        seen.push(os.name.clone());
        schemas.insert(os.name.clone(), object_schema(&os.fields));
    }
    for n in &os.nested {
        register_out_schema(n, schemas, seen);
    }
}

/// Resolve a return type to the object schema we register for it. A declared shape
/// projects its body; a bare model (or `full`) projects every stored column. The twin
/// of the client emitter's `out_struct`.
pub(crate) fn out_schema(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret: &RetType,
    root: Option<&RModel>,
    result_fallback: bool,
) -> OutSchema {
    if result_fallback {
        return OutSchema {
            name: "MutationResult".to_string(),
            fields: Vec::new(),
            is_result_fallback: true,
            nested: Vec::new(),
        };
    }
    let name = ret.ty.node.as_str();
    if name != "full" {
        if let Some(shape) = find_shape(decls, name) {
            return shape_out_schema(schema, decls, shape);
        }
    }
    match root {
        Some(m) => OutSchema {
            name: m.name.clone(),
            fields: model_fields(schema, m),
            is_result_fallback: false,
            nested: Vec::new(),
        },
        None => OutSchema {
            name: pascal(name),
            fields: Vec::new(),
            is_result_fallback: false,
            nested: Vec::new(),
        },
    }
}

/// The output-style object schema for a shape used as a `create … from` row input —
/// same projection a query returning that shape would emit, so input and output share
/// one component. `None` when the name is not a declared shape.
pub(crate) fn input_shape_out_schema(
    schema: &CheckedSchema,
    decls: &[Decl],
    name: &str,
) -> Option<OutSchema> {
    let shape = find_shape(decls, name)?;
    Some(shape_out_schema(schema, decls, shape))
}

/// Project a declared shape into its named `OutSchema` (body fields + referenced nests).
fn shape_out_schema(schema: &CheckedSchema, decls: &[Decl], shape: &Shape) -> OutSchema {
    let model = schema.model(&shape.from.node);
    let mut nested = Vec::new();
    let fields = shape_fields(
        schema,
        decls,
        &shape.body,
        model,
        &mut nested,
        &mut vec![shape.name.node.clone()],
    );
    OutSchema {
        name: shape.name.node.clone(),
        fields,
        is_result_fallback: false,
        nested,
    }
}

/// Project a shape body into `(field, schema, required)` triples. A `raw`…`` field
/// maps to the open-object `Json`; a to-**one** nest (`buyer { … }`) becomes an inline
/// nested object schema (recursively projected), required unless the relation is
/// optional. A to-**many** nest (`items { … }`) becomes an `array` of that object
/// schema (always present — an empty array when there are no children). A `field ->
/// Shape` nest `$ref`s the named shape's schema instead of inlining an object; the
/// referenced schema itself lands in `out` (registered once in `components.schemas`).
/// `stack` holds the shape names mid-expansion — the cycle guard (sema rejects
/// reference cycles; this keeps the emitter terminating regardless).
fn shape_fields(
    schema: &CheckedSchema,
    decls: &[Decl],
    body: &[ShapeField],
    model: Option<&RModel>,
    out: &mut Vec<OutSchema>,
    stack: &mut Vec<String>,
) -> Vec<(String, Value, bool)> {
    let mut fields = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => {
                let (ty, req) = reach_schema(schema, model, &[&id.node]);
                fields.push((id.node.clone(), ty, req));
            }
            ShapeField::Rename { out: alias, value } => {
                fields.push(rename_field(schema, model, alias, value));
            }
            ShapeField::Nest { field, body } => {
                if let Some(t) = nest_field(schema, decls, model, field, body, out, stack) {
                    fields.push(t);
                }
            }
            ShapeField::NestRef { field, shape } => {
                if let Some(t) = nest_ref_field(schema, decls, model, field, shape, out, stack) {
                    fields.push(t);
                }
            }
            // `out = edge.far { body }`: the distinct far side, hiding the junction — an
            // array of the far element object schema, always present (empty when there
            // are no far rows).
            ShapeField::Flatten {
                out: alias,
                path,
                body,
            } => {
                if let Some(target) = flatten_far_model(schema, model, path) {
                    let nested = shape_fields(schema, decls, body, Some(target), out, stack);
                    let arr = json!({ "type": "array", "items": object_schema(&nested) });
                    fields.push((alias.node.clone(), arr, true));
                }
            }
            ShapeField::Spread { .. } => unreachable!("spreads expanded before codegen"),
        }
    }
    fields
}

/// A renamed/derived field (`out = …`): a column path, a `raw`…`` open object, an
/// aggregate, or a per-row computed scalar.
fn rename_field(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    alias: &Ident,
    value: &ShapeValue,
) -> (String, Value, bool) {
    match value {
        ShapeValue::Path(p) => {
            let segs: Vec<&str> = p.segments.iter().map(|s| s.node.as_str()).collect();
            let (ty, req) = reach_schema(schema, model, &segs);
            (alias.node.clone(), ty, req)
        }
        ShapeValue::Raw(_) => (alias.node.clone(), json_schema(), false),
        // An aggregate: `count()` → a required integer; `avg` → a nullable number;
        // `sum`/`min`/`max` → the column's schema, nullable (an empty/all-null
        // group aggregates to null).
        ShapeValue::Agg(agg) => {
            let (ty, req) = match agg.func.node.as_str() {
                "count" => (primitive_schema(Primitive::Int), true),
                "avg" => (primitive_schema(Primitive::Float), false),
                _ => {
                    let base = agg.arg.as_ref().map_or_else(json_schema, |p| {
                        let segs: Vec<&str> = p.segments.iter().map(|s| s.node.as_str()).collect();
                        reach_schema(schema, model, &segs).0
                    });
                    (base, false)
                }
            };
            (alias.node.clone(), ty, req)
        }
        // A per-row derived scalar: its schema is inferred from the expression
        // (numeric family / text / the unified CASE branch type); `required` unless
        // it can be null (a CASE with a `null` branch).
        ShapeValue::Computed(expr) => {
            let (prim, optional) = crate::sql::dml::computed_result(schema, model, expr);
            let ty = prim.map_or_else(json_schema, primitive_schema);
            (alias.node.clone(), ty, !optional)
        }
    }
}

/// A `field { body }` relation nest: an inline object schema for a to-one edge (required
/// unless optional), or an array of the element object schema for a to-many edge.
fn nest_field(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: Option<&RModel>,
    field: &Ident,
    body: &[ShapeField],
    out: &mut Vec<OutSchema>,
    stack: &mut Vec<String>,
) -> Option<(String, Value, bool)> {
    if let Some((target, optional)) = to_one_relation(schema, model, &field.node) {
        let nested = shape_fields(schema, decls, body, Some(target), out, stack);
        Some((field.node.clone(), object_schema(&nested), !optional))
    } else if let Some(target) = to_many_relation(schema, model, &field.node) {
        // A to-many nest is an array of the element object schema; always present
        // (empty array when there are no children), so `required`.
        let nested = shape_fields(schema, decls, body, Some(target), out, stack);
        let arr = json!({ "type": "array", "items": object_schema(&nested) });
        Some((field.node.clone(), arr, true))
    } else {
        None
    }
}

/// A `field -> Shape` nest: registers the referenced shape in `out`, then `$ref`s it —
/// a bare `$ref` for a to-one edge, an array of it for a to-many edge.
fn nest_ref_field(
    schema: &CheckedSchema,
    decls: &[Decl],
    model: Option<&RModel>,
    field: &Ident,
    shape: &Ident,
    out: &mut Vec<OutSchema>,
    stack: &mut Vec<String>,
) -> Option<(String, Value, bool)> {
    let decl = find_shape(decls, &shape.node)?;
    if !stack.contains(&shape.node) {
        stack.push(shape.node.clone());
        let mut nested = Vec::new();
        let sfields = shape_fields(
            schema,
            decls,
            &decl.body,
            schema.model(&decl.from.node),
            &mut nested,
            stack,
        );
        stack.pop();
        out.push(OutSchema {
            name: shape.node.clone(),
            fields: sfields,
            is_result_fallback: false,
            nested,
        });
    }
    if let Some((_, optional)) = to_one_relation(schema, model, &field.node) {
        Some((field.node.clone(), schema_ref(&shape.node), !optional))
    } else if to_many_relation(schema, model, &field.node).is_some() {
        let arr = json!({ "type": "array", "items": schema_ref(&shape.node) });
        Some((field.node.clone(), arr, true))
    } else {
        None
    }
}

/// The far-side model of a flatten path (`edge.far`) — the last segment's relation
/// target. `None` on a malformed path (sema reports it).
fn flatten_far_model<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    path: &Path,
) -> Option<&'a RModel> {
    let mut cur = model?.name.clone();
    let mut out = None;
    for seg in &path.segments {
        let target = match schema.model(&cur)?.member(&seg.node).map(|m| &m.kind)? {
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => target,
            MemberKind::Scalar { .. } => return None,
        };
        out = schema.model(target);
        cur = target.clone();
    }
    out
}

/// The target model + `optional` of a **to-one** relation field, or `None` for a scalar,
/// an unknown field, or a to-**many** edge (a Forward is always to-one; an Inverse is
/// to-one only when its paired forward FK is unique — a one-to-one back edge, which may
/// be absent, hence optional). The OpenAPI twin of the client emitter's `to_one_relation`
/// and the SQL side's `enter_to_one`.
fn to_one_relation<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    field: &str,
) -> Option<(&'a RModel, bool)> {
    match model?.member(field).map(|m| &m.kind)? {
        MemberKind::Forward {
            target, optional, ..
        } => schema.model(target).map(|t| (t, *optional)),
        MemberKind::Inverse { target, via } => {
            let t = schema.model(target)?;
            t.is_unique(via).then_some((t, true))
        }
        MemberKind::Scalar { .. } => None,
    }
}

/// The target model of a to-**many** relation field (an Inverse collection — paired
/// forward FK not unique), or `None` for a scalar / to-one edge. The OpenAPI twin of the
/// SQL side's `to_many_edge`; the field emits as an array of the element object schema.
fn to_many_relation<'a>(
    schema: &'a CheckedSchema,
    model: Option<&RModel>,
    field: &str,
) -> Option<&'a RModel> {
    match model?.member(field).map(|m| &m.kind)? {
        MemberKind::Inverse { target, via } => {
            let t = schema.model(target)?;
            (!t.is_unique(via)).then_some(t)
        }
        _ => None,
    }
}

/// Every stored column of a bare-model return: scalars by their mapped schema, forward
/// FKs as a `uuid` string under the relation field name. Inverse edges store nothing.
fn model_fields(schema: &CheckedSchema, model: &RModel) -> Vec<(String, Value, bool)> {
    let mut fields = Vec::new();
    for mem in &model.members {
        match &mem.kind {
            MemberKind::Scalar {
                enum_name: Some(en),
                optional,
                many,
                ..
            } => fields.push((
                mem.name.clone(),
                wrap(enum_schema(schema, en), *many),
                !*optional,
            )),
            MemberKind::Scalar {
                ty, optional, many, ..
            } => fields.push((
                mem.name.clone(),
                wrap(primitive_schema(*ty), *many),
                !*optional,
            )),
            MemberKind::Forward {
                optional, target, ..
            } => {
                // The FK's wire schema mirrors the target model's primary-key type — a
                // uuid string, a ulid string, or a serial integer.
                fields.push((
                    mem.name.clone(),
                    fk_target_schema(schema, target),
                    !*optional,
                ));
            }
            MemberKind::Inverse { .. } => {}
        }
    }
    fields
}

/// The wire schema of a foreign key: the target model's primary-key schema — a uuid/ulid
/// string or serial integer for a single-column key, or a typed-property object (the
/// structured id) for a composite `@key`. Falls back to a uuid string when the target or
/// its id is unresolved (sema would have flagged it).
pub(crate) fn fk_target_schema(schema: &CheckedSchema, target: &str) -> Value {
    let Some(t) = schema.model(target) else {
        return uuid_schema();
    };
    if t.is_composite_key() {
        return composite_id_schema(schema, t);
    }
    match t.pk_member().map(|m| &m.kind) {
        Some(MemberKind::Scalar { ty, .. }) => primitive_schema(*ty),
        _ => uuid_schema(),
    }
}

/// The structured-id object schema of a composite-key model: one required property per key
/// part, each typed by the part's own schema (a relation part's target key, a scalar part's
/// primitive).
fn composite_id_schema(schema: &CheckedSchema, model: &based_sema::RModel) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in &model.key {
        let Some(mem) = model.member(f) else { continue };
        let part = match &mem.kind {
            MemberKind::Forward { target, .. } => fk_target_schema(schema, target),
            MemberKind::Scalar {
                enum_name: Some(en),
                ..
            } => enum_schema(schema, en),
            MemberKind::Scalar { ty, .. } => primitive_schema(*ty),
            MemberKind::Inverse { .. } => uuid_schema(),
        };
        props.insert(f.clone(), part);
        required.push(Value::String(f.clone()));
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String("object".to_string()));
    obj.insert("properties".to_string(), Value::Object(props));
    obj.insert("required".to_string(), Value::Array(required));
    Value::Object(obj)
}

/// An `object`-typed schema from `(field, schema, required)` triples. An empty body is
/// an open-ended object (a callable with no params posts `{}`).
fn object_schema(fields: &[(String, Value, bool)]) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (f, ty, req) in fields {
        props.insert(f.clone(), ty.clone());
        if *req {
            required.push(Value::String(f.clone()));
        }
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), json!("object"));
    if !required.is_empty() {
        obj.insert("required".to_string(), Value::Array(required));
    }
    obj.insert("properties".to_string(), Value::Object(props));
    Value::Object(obj)
}
