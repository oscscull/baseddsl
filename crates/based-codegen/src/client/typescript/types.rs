use super::*;
use based_ast::Primitive;
use based_sema::{EnumValue, REnum};

/// `export type Status = "pending" | "PAID";` — the enum's wire values.
pub(super) fn render_enum(e: &REnum) -> String {
    let values: Vec<String> = e
        .variants
        .iter()
        .map(|v| match &v.value {
            EnumValue::Str(s) => format!("{s:?}"),
            EnumValue::Int(i) => i.to_string(),
        })
        .collect();
    format!("\nexport type {} = {};\n", e.name, values.join(" | "))
}

/// A composite key's parts object.
pub(super) fn render_composite_id(schema: &CheckedSchema, model: &RModel) -> String {
    let mut out = format!(
        "\n/** The key of one `{0}` row. */\nexport interface {0}Id {{\n",
        model.name
    );
    for part in &model.key {
        if let Some(member) = model.member(part) {
            out.push_str(&format!(
                "  {part}: {};\n",
                value_type(schema, model, member)
            ));
        }
    }
    out.push_str("}\n");
    out
}

/// What a relation to `model` accepts: its key, or anything carrying it (a row).
pub(super) fn render_ref(schema: &CheckedSchema, model: &RModel) -> String {
    let name = &model.name;
    let ty = if model.is_composite_key() {
        format!("{name}Id")
    } else {
        let field = model.pk_field_names()[0];
        let key = key_type(schema, model);
        format!("{key} | {{ readonly {field}: {key} }}")
    };
    format!("\n/** Names one `{name}` row: its key, or a row of it. */\nexport type {name}Ref = {ty};\n")
}

/// The type naming one row of `model`.
pub(super) fn key_type(_schema: &CheckedSchema, model: &RModel) -> String {
    if model.is_composite_key() {
        format!("{}Id", model.name)
    } else {
        format!("Id<\"{}\">", model.name)
    }
}

/// A member's TypeScript type without nullability.
pub(super) fn value_type(schema: &CheckedSchema, model: &RModel, member: &RMember) -> String {
    match &member.kind {
        MemberKind::Scalar {
            raw_type: Some(_), ..
        } => "unknown".to_string(),
        MemberKind::Scalar {
            ty,
            many,
            enum_name,
            ..
        } => {
            let base =
                if !model.is_composite_key() && model.pk_field() == Some(member.name.as_str()) {
                    key_type(schema, model)
                } else if let Some(e) = enum_name {
                    e.clone()
                } else {
                    primitive(*ty).to_string()
                };
            if *many {
                format!("{base}[]")
            } else {
                base
            }
        }
        MemberKind::Forward { target, .. } => schema
            .model(target)
            .map_or_else(|| "unknown".to_string(), |t| key_type(schema, t)),
        MemberKind::Inverse { .. } => "unknown".to_string(),
    }
}

/// A member's TypeScript type as a row field: nullable columns admit `null`.
pub(super) fn row_type(schema: &CheckedSchema, model: &RModel, member: &RMember) -> String {
    let optional = match &member.kind {
        MemberKind::Scalar { optional, .. } | MemberKind::Forward { optional, .. } => *optional,
        MemberKind::Inverse { .. } => false,
    };
    let ty = value_type(schema, model, member);
    if optional {
        format!("{ty} | null")
    } else {
        ty
    }
}

fn primitive(p: Primitive) -> &'static str {
    match p {
        Primitive::Int | Primitive::Serial | Primitive::Float => "number",
        Primitive::Bool => "boolean",
        Primitive::Json => "unknown",
        Primitive::Text
        | Primitive::Timestamp
        | Primitive::Date
        | Primitive::Time
        | Primitive::Bytes
        | Primitive::Uuid
        | Primitive::Id
        | Primitive::Ulid
        | Primitive::Decimal { .. } => "string",
    }
}

/// Whether a member reads back as a boolean (stored 0/1 on some databases).
pub(super) fn is_bool(member: &RMember) -> bool {
    matches!(
        member.kind,
        MemberKind::Scalar {
            ty: Primitive::Bool,
            many: false,
            ..
        }
    )
}
