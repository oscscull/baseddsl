use super::*;

/// The generated `<M>Id` struct for a composite-key model: one typed field per key part
/// (a relation part keeps its target's typed id, a scalar part its own type). Serde
/// (de)serializes it as a JSON object of the parts.
pub(super) fn composite_id_struct(schema: &CheckedSchema, m: &RModel) -> String {
    let mut s = format!(
        "#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]\npub struct {}Id {{\n",
        m.name
    );
    for f in &m.key {
        let Some(mem) = m.member(f) else { continue };
        let ty = match &mem.kind {
            MemberKind::Forward { target, .. } => id_type(schema, target),
            MemberKind::Scalar {
                enum_name: Some(en),
                ..
            } => en.clone(),
            MemberKind::Scalar { ty, .. } => primitive(*ty).to_string(),
            MemberKind::Inverse { .. } => "Json".to_string(),
        };
        s.push_str(&format!("    pub {f}: {ty},\n"));
    }
    s.push_str("}\n");
    s
}

/// A query's return wrapper: stream -> `RowStream<T>`, paginated -> `Page<T>`,
/// many -> `Vec<T>`, single -> `Option<T>` (a `get` may match nothing).
pub(super) fn query_output(rq: &RQuery, ty: &str) -> String {
    if rq.stream {
        format!("RowStream<{ty}>")
    } else if rq.paginated {
        format!("Page<{ty}>")
    } else if rq.many {
        format!("Vec<{ty}>")
    } else {
        format!("Option<{ty}>")
    }
}

/// Resolve a return type to the struct we emit for it. A shape projects its body;
/// a bare model (or `full`) projects every stored column.
pub(super) fn out_struct(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret: &RetType,
    root: Option<&RModel>,
) -> OutStruct {
    let name = ret.ty.node.as_str();
    // A declared shape: its struct is the projected body against the shape model.
    if name != "full" {
        if let Some(shape) = find_shape(decls, name) {
            let model = schema.model(&shape.from.node);
            return build_struct(
                schema,
                decls,
                name.to_string(),
                &shape.body,
                model,
                &mut vec![name.to_string()],
            );
        }
    }
    // `full` or a bare model: every stored column of the resolved model.
    match root {
        Some(m) => OutStruct {
            name: m.name.clone(),
            fields: model_fields(schema, m),
            nested: Vec::new(),
        },
        // Unresolvable (sema would have flagged it) — an empty struct keeps the
        // emitted module compiling with a valid type.
        None => OutStruct {
            name: pascal(name),
            fields: Vec::new(),
            nested: Vec::new(),
        },
    }
}

/// Build one output struct from a shape body: `(field, type)` pairs plus the
/// auxiliary structs for its to-one nested sub-objects. A `raw`…`` field maps to
/// `Json`; a to-one nest (`buyer { … }`) becomes a nested struct named
/// `<Parent><Field>` and the field takes that type (`Option<…>` when the relation
/// is optional). A to-many nest (`items { … }`) becomes a nested struct wrapped in
/// `Vec<…>`. A `field -> Shape` nest references the named shape's own struct,
/// so every site shares one nominal type;
/// `stack` holds the shape names mid-expansion (a cycle guard — sema rejects
/// reference cycles, this keeps the emitter terminating regardless).
pub(super) fn build_struct(
    schema: &CheckedSchema,
    decls: &[Decl],
    name: String,
    body: &[ShapeField],
    model: Option<&RModel>,
    stack: &mut Vec<String>,
) -> OutStruct {
    let mut fields = Vec::new();
    let mut nested = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => {
                fields.push((id.node.clone(), reach_type(schema, model, &[&id.node])));
            }
            ShapeField::Rename { out, value } => {
                fields.push((out.node.clone(), rename_type(schema, model, value)));
            }
            ShapeField::Nest { field, body } => {
                nest_field(
                    schema,
                    decls,
                    &name,
                    field,
                    body,
                    model,
                    stack,
                    &mut fields,
                    &mut nested,
                );
            }
            ShapeField::NestRef { field, shape } => {
                nestref_field(
                    schema,
                    decls,
                    field,
                    shape,
                    model,
                    stack,
                    &mut fields,
                    &mut nested,
                );
            }
            ShapeField::Flatten { out, path, body } => {
                flatten_field(
                    schema,
                    decls,
                    &name,
                    out,
                    path,
                    body,
                    model,
                    stack,
                    &mut fields,
                    &mut nested,
                );
            }
            ShapeField::Spread { .. } => unreachable!("spreads expanded before codegen"),
        }
    }
    OutStruct {
        name,
        fields,
        nested,
    }
}

/// A `field { … }` nest: a to-one relation becomes a `<Parent><Field>` nested struct (the
/// field `Option<…>` when the relation is optional); a to-many becomes that struct wrapped
/// in `Vec<…>`.
#[allow(clippy::too_many_arguments)]
fn nest_field(
    schema: &CheckedSchema,
    decls: &[Decl],
    name: &str,
    field: &Ident,
    body: &[ShapeField],
    model: Option<&RModel>,
    stack: &mut Vec<String>,
    fields: &mut Vec<(String, String)>,
    nested: &mut Vec<OutStruct>,
) {
    if let Some((target, optional)) = to_one_relation(schema, model, &field.node) {
        let sub_name = format!("{name}{}", pascal(&field.node));
        let sub = build_struct(schema, decls, sub_name.clone(), body, Some(target), stack);
        let ty = if optional {
            format!("Option<{sub_name}>")
        } else {
            sub_name
        };
        fields.push((field.node.clone(), ty));
        nested.push(sub);
    } else if let Some(target) = to_many_relation(schema, model, &field.node) {
        // A to-many nest is a JSON array of the element struct: `Vec<Sub>`.
        let sub_name = format!("{name}{}", pascal(&field.node));
        let sub = build_struct(schema, decls, sub_name.clone(), body, Some(target), stack);
        fields.push((field.node.clone(), format!("Vec<{sub_name}>")));
        nested.push(sub);
    }
}

/// A `field -> Shape` nest: references the named shape's own struct (built once, deduped by
/// name), so every referencing site shares one nominal type.
#[allow(clippy::too_many_arguments)]
fn nestref_field(
    schema: &CheckedSchema,
    decls: &[Decl],
    field: &Ident,
    shape: &Spanned<String>,
    model: Option<&RModel>,
    stack: &mut Vec<String>,
    fields: &mut Vec<(String, String)>,
    nested: &mut Vec<OutStruct>,
) {
    let Some(decl) = find_shape(decls, &shape.node) else {
        return;
    };
    if !stack.contains(&shape.node) {
        // Build the referenced shape's own struct (emitted once,
        // deduped by name across every referencing site).
        stack.push(shape.node.clone());
        let sub = build_struct(
            schema,
            decls,
            shape.node.clone(),
            &decl.body,
            schema.model(&decl.from.node),
            stack,
        );
        stack.pop();
        nested.push(sub);
    }
    if let Some((_, optional)) = to_one_relation(schema, model, &field.node) {
        let ty = if optional {
            format!("Option<{}>", shape.node)
        } else {
            shape.node.clone()
        };
        fields.push((field.node.clone(), ty));
    } else if to_many_relation(schema, model, &field.node).is_some() {
        fields.push((field.node.clone(), format!("Vec<{}>", shape.node)));
    }
}

/// A `out = edge.far { … }` flatten: the distinct far-side rows, hiding the junction — a
/// `Vec<Sub>` of the far model's element struct, like a to-many nest.
#[allow(clippy::too_many_arguments)]
fn flatten_field(
    schema: &CheckedSchema,
    decls: &[Decl],
    name: &str,
    out: &Ident,
    path: &Path,
    body: &[ShapeField],
    model: Option<&RModel>,
    stack: &mut Vec<String>,
    fields: &mut Vec<(String, String)>,
    nested: &mut Vec<OutStruct>,
) {
    if let Some(target) = flatten_far_model(schema, model, path) {
        let sub_name = format!("{name}{}", pascal(&out.node));
        let sub = build_struct(schema, decls, sub_name.clone(), body, Some(target), stack);
        fields.push((out.node.clone(), format!("Vec<{sub_name}>")));
        nested.push(sub);
    }
}

/// Emit an output struct and its to-one nested aux structs, deduped by name across
/// callables (a shape shared by two queries is one struct).
pub(super) fn emit_struct(out: &mut String, s: &OutStruct, seen: &mut Vec<String>) {
    if seen.contains(&s.name) {
        return;
    }
    seen.push(s.name.clone());
    out.push('\n');
    out.push_str(&render_struct(&s.name, &s.fields));
    for n in &s.nested {
        emit_struct(out, n, seen);
    }
}

/// Every stored column of a bare-model return: scalars by their type (the `id`
/// column as this model's typed id), forward FKs as the target's typed id under the
/// relation field name (matching the SELECT alias). Inverse edges store nothing, so
/// they are omitted.
fn model_fields(schema: &CheckedSchema, model: &RModel) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for mem in &model.members {
        match &mem.kind {
            // The single-column primary-key column carries this model's typed id — the
            // `id` field or a `@key(field)` natural key, whatever its declared type. A
            // composite key has no single id field; its parts fall through as ordinary
            // scalar/relation fields.
            MemberKind::Scalar { optional, many, .. }
                if !model.is_composite_key() && model.pk_field() == Some(mem.name.as_str()) =>
            {
                fields.push((
                    mem.name.clone(),
                    wrap(&id_type(schema, &model.name), *optional, *many),
                ));
            }
            MemberKind::Scalar {
                enum_name: Some(en),
                optional,
                many,
                ..
            } => fields.push((mem.name.clone(), wrap(en, *optional, *many))),
            MemberKind::Scalar {
                ty, optional, many, ..
            } => fields.push((mem.name.clone(), wrap(primitive(*ty), *optional, *many))),
            MemberKind::Forward {
                target, optional, ..
            } => fields.push((
                mem.name.clone(),
                wrap(&id_type(schema, target), *optional, false),
            )),
            MemberKind::Inverse { .. } => {}
        }
    }
    fields
}
