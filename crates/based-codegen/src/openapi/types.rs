//! DSL type -> JSON Schema mapping: primitives, enums, FK/uuid strings, and dotted
//! field-path resolution against a model.

use super::*;

/// Resolve a dotted field path against `model` to a `(schema, required)` pair. A scalar
/// terminal is its mapped schema (carrying `many`, and `required = !optional`); a
/// relation terminal is a `uuid` string; intermediate relation hops walk to the target
/// model. Unknown paths (sema already flagged) fall back to the open-object `Json`.
pub(crate) fn reach_schema(
    schema: &CheckedSchema,
    model: Option<&RModel>,
    path: &[&str],
) -> (Value, bool) {
    let Some(mut cur) = model else {
        return (uuid_schema(), true);
    };
    let n = path.len();
    for (i, seg) in path.iter().enumerate() {
        let last = i + 1 == n;
        match cur.member(seg).map(|m| &m.kind) {
            Some(MemberKind::Scalar {
                enum_name: Some(en),
                optional,
                many,
                ..
            }) => return (wrap(enum_schema(schema, en), *many), !*optional),
            Some(MemberKind::Scalar {
                ty, optional, many, ..
            }) => return (wrap(primitive_schema(*ty), *many), !*optional),
            Some(MemberKind::Forward {
                target, optional, ..
            }) => {
                if last {
                    return (fk_target_schema(schema, target), !*optional);
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return (json_schema(), false),
                }
            }
            Some(MemberKind::Inverse { target, .. }) => {
                if last {
                    // Terminal to-many reach: an array of the target's key schema.
                    return (
                        json!({ "type": "array", "items": fk_target_schema(schema, target) }),
                        true,
                    );
                }
                match schema.model(target) {
                    Some(m) => cur = m,
                    None => return (json_schema(), false),
                }
            }
            None => return (json_schema(), false),
        }
    }
    (json_schema(), false)
}

/// Wrap a base schema for a to-many field: `many` -> `{ type: array, items: base }`.
pub(crate) fn wrap(base: Value, many: bool) -> Value {
    if many {
        json!({ "type": "array", "items": base })
    } else {
        base
    }
}

/// A primitive's JSON Schema (mirrors the DDL/client mapping). `uuid`/date/
/// timestamp carry the standard OpenAPI `format` so a generator can pick a rich type.
pub(crate) fn primitive_schema(p: Primitive) -> Value {
    match p {
        Primitive::Text => json!({ "type": "string" }),
        Primitive::Int => json!({ "type": "integer", "format": "int64" }),
        Primitive::Bool => json!({ "type": "boolean" }),
        Primitive::Timestamp => json!({ "type": "string", "format": "date-time" }),
        Primitive::Date => json!({ "type": "string", "format": "date" }),
        // `time` is an RFC 3339 partial-time string; `bytes` a base64 string (OpenAPI
        // 3.1 `format: byte`), never a raw JSON byte array.
        Primitive::Time => json!({ "type": "string", "format": "time" }),
        Primitive::Bytes => json!({ "type": "string", "format": "byte" }),
        Primitive::Json => json_schema(),
        Primitive::Uuid | Primitive::Id => uuid_schema(),
        // A `ulid` is a 26-char string (not a uuid); a `serial` id is a JSON integer.
        Primitive::Ulid => json!({ "type": "string" }),
        Primitive::Serial => json!({ "type": "integer", "format": "int64" }),
        Primitive::Float => json!({ "type": "number", "format": "double" }),
        // A decimal is a string on the wire (lossless), never a JSON float.
        Primitive::Decimal { .. } => json!({ "type": "string", "format": "decimal" }),
    }
}

/// A param/field base schema: a primitive, or a model reference as the `uuid` FK the
/// wire carries.
pub(crate) fn base_schema(b: &BaseType) -> Value {
    match b {
        BaseType::Primitive(p) => primitive_schema(*p),
        BaseType::Model(_) => uuid_schema(),
        // An opaque `raw(…)` value is an unmodelled string on the wire.
        BaseType::Raw(_) => serde_json::json!({ "type": "string" }),
    }
}

/// The `uuid`-string schema (a relation/id FK on the wire).
pub(crate) fn uuid_schema() -> Value {
    json!({ "type": "string", "format": "uuid" })
}

/// An enum column's schema, constrained to the enum's wire values: a string enum is
/// `{ "type": "string", "enum": ["pending", …] }`; an int enum is
/// `{ "type": "integer", "enum": [0, …] }`. Falls back to an open string if the enum is
/// somehow unresolved (sema would have flagged it).
pub(crate) fn enum_schema(schema: &CheckedSchema, name: &str) -> Value {
    use based_sema::{EnumKind, EnumValue};
    let Some(e) = schema.enum_(name) else {
        return json!({ "type": "string" });
    };
    match e.kind {
        EnumKind::Str => {
            let values: Vec<&str> = e
                .variants
                .iter()
                .map(|v| match &v.value {
                    EnumValue::Str(s) => s.as_str(),
                    EnumValue::Int(_) => v.name.as_str(),
                })
                .collect();
            json!({ "type": "string", "enum": values })
        }
        EnumKind::Int => {
            let values: Vec<i64> = e
                .variants
                .iter()
                .map(|v| match &v.value {
                    EnumValue::Int(n) => *n,
                    EnumValue::Str(_) => 0,
                })
                .collect();
            json!({ "type": "integer", "enum": values })
        }
    }
}

/// The open-object `json` schema: any JSON value (a `json` column or a `raw`…`` field).
pub(crate) fn json_schema() -> Value {
    // `true` is JSON Schema for "anything"; OpenAPI 3.1 accepts it.
    Value::Bool(true)
}

/// A primitive's name for the `x-ctx-requires` listing (matches the sema rendering).
pub(crate) fn primitive_name(p: Primitive) -> &'static str {
    match p {
        Primitive::Text => "text",
        Primitive::Int => "int",
        Primitive::Bool => "bool",
        Primitive::Timestamp => "timestamp",
        Primitive::Date => "date",
        Primitive::Time => "time",
        Primitive::Bytes => "bytes",
        Primitive::Json => "json",
        Primitive::Uuid => "uuid",
        Primitive::Id => "Id",
        Primitive::Ulid => "ulid",
        Primitive::Serial => "serial",
        Primitive::Float => "float",
        Primitive::Decimal { .. } => "decimal",
    }
}
