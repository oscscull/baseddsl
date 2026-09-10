use super::*;

/// Resolve `@key(f1, f2, …)` — the nominated primary key. Returns the field names in list
/// order (empty when no `@key`). One field is a natural single-column key; two or more form
/// a composite `PRIMARY KEY` over those columns in list order. Either way no surrogate `id`
/// is synthesized. Each nominated field must exist and be a required, single-valued
/// column — a scalar, or a to-one relation whose FK column carries the key. `@key`
/// on a keyless (`@no_id`) model is contradictory; an empty `@key()` and a
/// field named twice are rejected. A rejected key resolves to empty so the model
/// still needs an `id` (the ordinary path), never a half-formed key.
pub(crate) fn resolve_key(m: &Model, members: &[RMember], no_id: bool, sink: &mut Sink) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let mut key_span: Option<Span> = None;
    for d in &m.decorators {
        if d.name.node != "key" {
            continue;
        }
        key_span = Some(d.span);
        for a in &d.args {
            match a {
                DecoArg::Ident(id) => fields.push(id.node.clone()),
                DecoArg::Path(p) if p.segments.len() == 1 => {
                    fields.push(p.segments[0].node.clone());
                }
                _ => {}
            }
        }
    }
    let Some(span) = key_span else {
        return Vec::new();
    };
    // A model is keyed either by a nominated column or declared keyless, never both.
    if no_id {
        sink.error_note(
            code::KEY_WITH_NO_ID,
            span,
            format!("`{}` carries both `@key` and `@no_id`", m.name.node),
            "`@key` nominates a primary key; `@no_id` declares the table keyless — a model is one or the other",
        );
        return Vec::new();
    }
    if fields.is_empty() {
        sink.error_note(
            code::KEY_EMPTY,
            span,
            format!("`@key()` on `{}` names no field", m.name.node),
            "a primary key must nominate at least one declared column: `@key(field, …)`",
        );
        return Vec::new();
    }
    let mut ok = true;
    // Each key column appears once — a repeated field is a copy-paste slip, not a key.
    let mut seen: Vec<&str> = Vec::new();
    for f in &fields {
        if seen.contains(&f.as_str()) {
            sink.error_note(
                code::KEY_DUPLICATE,
                span,
                format!("`@key` on `{}` names `{f}` twice", m.name.node),
                "each key column appears once, in the order it should index",
            );
            ok = false;
        }
        seen.push(f);
    }
    // Each nominated field must exist and be usable as a primary-key column.
    for f in &fields {
        match members
            .iter()
            .find(|mem| &mem.name == f)
            .map(|mem| &mem.kind)
        {
            None => {
                sink.error(
                    code::KEY_UNKNOWN_FIELD,
                    span,
                    format!(
                        "`@key({f})` names field `{f}`, which `{}` does not declare",
                        m.name.node
                    ),
                );
                ok = false;
            }
            // A required, single-valued scalar is keyable; so is a required to-one relation
            // — its FK column carries the key (the junction pattern `@key(order, product)`).
            Some(
                MemberKind::Scalar {
                    optional: false,
                    many: false,
                    raw_type: None,
                    ..
                }
                | MemberKind::Forward {
                    optional: false,
                    custom_on: None,
                    ..
                },
            ) => {}
            Some(_) => {
                sink.error_note(
                    code::KEY_UNSUITABLE,
                    span,
                    format!("`@key({f})` — `{f}` cannot be a primary-key column"),
                    "a key column must be a required (non-optional, single-valued) scalar or to-one relation",
                );
                ok = false;
            }
        }
    }
    if ok {
        fields
    } else {
        Vec::new()
    }
}

/// Validate primary-key generation-strategy types. `serial`/`ulid` are PK
/// generation strategies: valid only as the `id` column's type, never on an ordinary
/// column. And a bare numeric type (`int`/`float`/`decimal`) as the `id` PK is
/// rejected — a DB-generated integer key must be spelled `serial` so its
/// generation is visible (principles 1, 2, 8); an app-owned natural key stays a string
/// (`text`/`uuid`). A `@no_id` model has no `id`, so neither rule fires on it.
pub(crate) fn check_pk_types(m: &Model, key: &[String], sink: &mut Sink) {
    // A `serial` part is legal only inside a *composite* `@key(f1, f2, …)` (≥2 columns) —
    // there it is a DB-generated key column the engine omits on create and reads back.
    let composite = key.len() >= 2;
    let mut serial_parts = 0usize;
    for mem in &m.members {
        let Member::Field(f) = mem else { continue };
        let is_id = f.name.node == "id";
        let BaseType::Primitive(p) = &f.ty.base else {
            continue;
        };
        let in_composite_key = composite && key.iter().any(|k| k == &f.name.node);
        match (is_id, p) {
            // A `serial` part of a composite key is DB-generated and legal. A
            // table has at most one auto-increment column, so a second serial part is an
            // error.
            (false, Primitive::Serial) if in_composite_key => {
                serial_parts += 1;
                if serial_parts > 1 {
                    sink.error_note(
                        code::PK_MULTIPLE_SERIAL,
                        f.ty.span,
                        format!("`{}` names more than one `serial` key part", m.name.node),
                        "a table has at most one DB-generated (`serial`) column; make the extra key parts app-supplied",
                    );
                }
            }
            // A PK strategy type on a non-`id`, non-composite-key column — not an ordinary
            // column type.
            (false, Primitive::Serial | Primitive::Ulid) => sink.error_note(
                code::PK_STRATEGY_MISPLACED,
                f.ty.span,
                format!(
                    "`{}` is a primary-key generation strategy, not a column type",
                    if matches!(p, Primitive::Serial) {
                        "serial"
                    } else {
                        "ulid"
                    }
                ),
                if matches!(p, Primitive::Serial) {
                    "`serial` is valid as the `id` primary key, or as a part of a composite `@key(f1, f2, …)`"
                } else {
                    "`ulid` is valid only as the `id` primary key's type"
                },
            ),
            // A bare numeric `id` — its generation (app-set vs DB-owned) is invisible.
            (true, Primitive::Int | Primitive::Float | Primitive::Decimal { .. }) => sink
                .error_note(
                    code::PK_BARE_INT,
                    f.ty.span,
                    format!("`id: {}` is not a legal primary key", prim_source_name(p)),
                    "a DB-generated integer key is spelled `id: serial` (its generation is \
                     consequential and must be written); an app-owned key stays a string \
                     (`id: text`/`id: uuid`)",
                ),
            _ => {}
        }
    }
}

/// A primitive's source spelling, for a diagnostic.
pub(crate) fn prim_source_name(p: &Primitive) -> &'static str {
    match p {
        Primitive::Int => "int",
        Primitive::Float => "float",
        Primitive::Decimal { .. } => "decimal",
        _ => "?",
    }
}
