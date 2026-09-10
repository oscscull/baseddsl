use super::*;

/// Build an unvalidated `RModel` from one AST model: implicit `id`, columns, and
/// relations classified forward vs. inverse. No cross-model checks yet. `enums` maps each
/// declared enum name to its kind, so an UpperCamel field type resolving to an enum is a
/// scalar column (text for a string enum, integer for an int enum) rather than a relation.
pub fn skeleton(m: &Model, enums: &HashMap<String, EnumKind>, sink: &mut Sink) -> RModel {
    let mut members: Vec<RMember> = Vec::new();
    let mut seen: HashMap<String, Span> = HashMap::new();

    for mem in &m.members {
        let (name, span) = match mem {
            Member::Field(f) => (&f.name.node, f.name.span),
            Member::Generated(g) => (&g.name.node, g.name.span),
            _ => continue,
        };
        if seen.contains_key(name) {
            sink.error(
                code::DUP_FIELD,
                span,
                format!("duplicate field `{}` in model `{}`", name, m.name.node),
            );
            continue;
        }
        seen.insert(name.clone(), span);
        match mem {
            Member::Field(f) => {
                // Field `(default now())` functions are validated against the closed set.
                for m in &f.modifiers {
                    if let Modifier::Default(dv) = m {
                        resolve::check_default(dv, sink);
                    }
                }
                members.push(RMember {
                    name: f.name.node.clone(),
                    span: f.name.span,
                    kind: classify(f, enums),
                    was: f.was.as_ref().map(|w| w.node.clone()),
                    sort: f.sort.clone().unwrap_or_default(),
                });
            }
            // A generated column becomes a real scalar column carrying its expression; its
            // type/nullability are inferred after every member is built (below), since the
            // expression references the model's own columns.
            Member::Generated(g) => members.push(generated_member(g)),
            _ => {}
        }
    }

    // Generated columns: validate the expression (same-row-only, operand families) and
    // infer each one's type + nullability from the row's own columns.
    resolve_generated(&mut members, sink);

    // `@no_id("reason")` opts a genuinely keyless legacy table out of the primary key
    // (the reason is mandatory, so the PR shows why —).
    let no_id = model_no_id(m, sink);

    // `@key(field, …)` nominates a declared field as the primary key (no surrogate `id`).
    let key = resolve_key(m, &members, no_id, sink);

    // PK generation-type check needs the resolved key (a `serial` part is legal only inside
    // a composite `@key`).
    check_pk_types(m, &key, sink);

    // `@no_fk` opts the whole table out of FK constraints (reason checked, if required,
    // in the manifest-dependent divergence pass).
    let no_fk = model_no_fk(m);

    // A model's primary key is load-bearing and written in source. A model that
    // declares no `id` field is an error with a one-key autofix; the `id`
    // member is still synthesized so the rest of resolution + codegen has a PK to
    // key on, but the source must name it. A `@no_id` model legitimately has none, and
    // a `@key` model nominates an existing column instead.
    if !seen.contains_key("id") && !no_id && key.is_empty() {
        sink.error_fix(
            code::NO_ID,
            m.name.span,
            format!("model `{}` declares no `id`", m.name.node),
            "every model needs a primary key — add an `id: Id` field, or `@no_id(\"reason\")` for a keyless legacy table",
            m.name.node.clone(),
            "id: Id",
        );
        members.insert(
            0,
            RMember {
                name: "id".to_string(),
                span: m.name.span,
                kind: MemberKind::Scalar {
                    ty: Primitive::Id,
                    optional: false,
                    many: false,
                    column: "id".to_string(),
                    unique: false, // PK, expressed as PRIMARY KEY not a UNIQUE constraint
                    default: None, // engine-generated on insert , no SQL default
                    enum_name: None,
                    raw_type: None,
                    generated: None,
                },
                was: None,
                sort: Vec::new(),
            },
        );
    }

    let table = table_name(m, sink);
    let schema = model_schema(m, sink);
    RModel {
        name: m.name.node.clone(),
        span: m.span,
        table,
        schema,
        members,
        soft_delete: None,
        sort: Vec::new(),
        scope: None,
        scope_alts: Vec::new(),
        created: None,
        updated: None,
        indexes: Vec::new(),
        no_id,
        no_fk: no_fk.is_some(),
        no_fk_reason: no_fk.as_ref().and_then(|(r, _)| r.clone()),
        no_fk_span: no_fk.map(|(_, s)| s),
        unique_cols: Vec::new(),
        was: model_was(m),
        key,
    }
}
