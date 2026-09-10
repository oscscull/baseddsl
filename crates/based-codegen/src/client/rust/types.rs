use super::*;

/// The Rust type of an entity id: a phantom-typed `Id<entity::M>` newtype for a
/// single-column key (distinct per model so ids can't be swapped), or the generated
/// per-part struct `<M>Id` for a composite `@key(f1, f2, …)` model.
pub(super) fn id_type(schema: &CheckedSchema, entity: &str) -> String {
    if schema.model(entity).is_some_and(RModel::is_composite_key) {
        format!("{entity}Id")
    } else {
        format!("Id<entity::{entity}>")
    }
}

/// Resolve a dotted field path against `model` to a Rust type. A scalar terminal
/// is its mapped primitive (carrying `optional`/`many`); a relation terminal is
/// the FK `Uuid`; intermediate relation hops walk to the target model. Unknown
/// paths (sema already flagged) fall back to `Json` so the module still compiles.
pub(super) fn reach_type(schema: &CheckedSchema, model: Option<&RModel>, path: &[&str]) -> String {
    let Some(mut cur) = model else {
        return "Uuid".to_string();
    };
    let n = path.len();
    for (i, seg) in path.iter().enumerate() {
        let last = i + 1 == n;
        let is_pk = !cur.is_composite_key() && cur.pk_field() == Some(*seg);
        match cur.member(seg).map(|m| &m.kind) {
            // The model's own single-column primary key is that model's typed id.
            Some(MemberKind::Scalar { optional, many, .. }) if is_pk => {
                return wrap(&id_type(schema, &cur.name), *optional, *many)
            }
            Some(MemberKind::Scalar {
                enum_name: Some(en),
                optional,
                many,
                ..
            }) => return wrap(en, *optional, *many),
            Some(MemberKind::Scalar {
                ty, optional, many, ..
            }) => return wrap(primitive(*ty), *optional, *many),
            Some(MemberKind::Forward {
                target, optional, ..
            }) => {
                if last {
                    return wrap(&id_type(schema, target), *optional, false);
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return "Json".to_string(),
                }
            }
            Some(MemberKind::Inverse { target, .. }) => {
                if last {
                    // Terminal to-many reach: a collection of the target's typed ids.
                    return format!("Vec<{}>", id_type(schema, target));
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return "Json".to_string(),
                }
            }
            None => return "Json".to_string(),
        }
    }
    "Json".to_string()
}

/// The Rust type of an aggregate shape field. `count()` is a non-null `i64`; `avg`
/// is `Option<f64>`; `sum`/`min`/`max` are `Option<column-type>` — nullable because an
/// empty or all-null group aggregates to null.
fn agg_type(schema: &CheckedSchema, model: Option<&RModel>, agg: &AggCall) -> String {
    match agg.func.node.as_str() {
        "count" => "i64".to_string(),
        "avg" => "Option<f64>".to_string(),
        _ => {
            let base = agg
                .arg
                .as_ref()
                .and_then(|p| col_primitive(schema, model, p))
                .map_or("Json", primitive);
            format!("Option<{base}>")
        }
    }
}

/// The Rust type of an `out = value` shape field: a reach path's column type, `Json`
/// for a raw expression, an aggregate's type, or a computed expression's inferred type.
pub(super) fn rename_type(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    value: &ShapeValue,
) -> String {
    match value {
        ShapeValue::Path(p) => {
            let segs: Vec<&str> = p.segments.iter().map(|s| s.node.as_str()).collect();
            reach_type(schema, model, &segs)
        }
        // A raw SQL expression has no statically known type -> `Json`.
        ShapeValue::Raw(_) => "Json".to_string(),
        // An aggregate: `count()` → `i64`, `avg` → `Option<f64>`, `sum`/`min`/`max` →
        // `Option<column-type>` (an empty/all-null group aggregates to null).
        ShapeValue::Agg(agg) => agg_type(schema, model, agg),
        // A per-row derived scalar: type inferred from the expression (numeric family /
        // text / the unified CASE branch type).
        ShapeValue::Computed(expr) => computed_type(schema, model, expr),
    }
}

/// The Rust type of a computed shape field (`out = <expr>`), inferred from the
/// expression via the shared [`crate::sql::dml::computed_result`]. An unknown/opaque
/// leaf falls back to `Json`; a nullable result (a CASE with a `null` branch) is
/// `Option<…>`.
fn computed_type(schema: &CheckedSchema, model: Option<&RModel>, expr: &ShapeExpr) -> String {
    let (prim, optional) = crate::sql::dml::computed_result(schema, model, expr);
    let base = prim.map_or_else(|| "Json".to_string(), |p| primitive(p).to_string());
    if optional {
        format!("Option<{base}>")
    } else {
        base
    }
}

/// The primitive a dotted column path terminates on, walking relations to the target.
/// `None` for a path that doesn't land on a scalar (sema already flagged it).
fn col_primitive(schema: &CheckedSchema, model: Option<&RModel>, path: &Path) -> Option<Primitive> {
    let mut cur = model?;
    let n = path.segments.len();
    for (i, seg) in path.segments.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(&seg.node).map(|m| &m.kind)? {
            MemberKind::Scalar { ty, .. } if last => return Some(*ty),
            MemberKind::Scalar { .. } => return None,
            MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. } => {
                if last {
                    return None;
                }
                cur = schema.model(target)?;
            }
        }
    }
    None
}

/// Wrap a base type: to-many -> `Vec<base>`, then optional -> `Option<…>`.
pub(super) fn wrap(base: &str, optional: bool, many: bool) -> String {
    let inner = if many {
        format!("Vec<{base}>")
    } else {
        base.to_string()
    };
    if optional {
        format!("Option<{inner}>")
    } else {
        inner
    }
}

/// A primitive type name as its Rust alias (see the module type-mapping table).
pub(super) fn primitive(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "String",
        Primitive::Int => "i64",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "Timestamp",
        Primitive::Date => "Date",
        Primitive::Time => "Time",
        // A `bytes` value rides the wire as a **base64** string, so the client carries
        // it as a `String` (its base64 text) — no extra dep, lossless.
        Primitive::Bytes => "Bytes",
        Primitive::Json => "Json",
        Primitive::Uuid | Primitive::Id | Primitive::Ulid => "Uuid",
        // A bare `serial` value (not the typed `Id<entity::M>`, which the id-resolution
        // path uses) rides the wire as a JSON number.
        Primitive::Serial => "i64",
        Primitive::Float => "f64",
        // A decimal rides the wire as a JSON string; the `serde-str` feature (in the
        // consumer's Cargo.toml) makes `rust_decimal::Decimal` (de)serialize as a
        // string, so no digit is lost. Referenced by full path — a schema with no
        // decimal never mentions `rust_decimal`, so the dep is needed only when used.
        Primitive::Decimal { .. } => "rust_decimal::Decimal",
    }
}

/// A param/field base type: a primitive by its alias, a model reference as the
/// `Uuid` FK the wire carries.
pub(super) fn base_type(b: &BaseType) -> &'static str {
    match b {
        BaseType::Primitive(p) => primitive(*p),
        BaseType::Model(_) => "Uuid",
        // An opaque `raw(…)` value crosses the wire as a string; the engine models
        // nothing about it. (Only a model field may carry one, so this is the
        // shape-projection path.)
        BaseType::Raw(_) => "String",
    }
}
