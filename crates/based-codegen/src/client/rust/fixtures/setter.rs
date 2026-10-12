use super::*;

/// Wire-string types a setter accepts through `Into`, so `&str` works.
const STRING_TYPES: &[&str] = &["String", "Timestamp", "Date", "Time", "Bytes", "Uuid"];

/// One builder setter: a typed value for a column, any row reference for a relation.
pub(super) fn render_setter(
    schema: &CheckedSchema,
    model: &RModel,
    field: &FixtureField<'_>,
) -> String {
    let method = setter_name(field.name);
    let key = field.name;
    match &field.value {
        FieldValue::Link { target, .. } => format!(
            "    pub fn {method}(mut self, value: impl RowRef<{ty}>) -> Self {{\n        \
             self.row.insert(\"{key}\".to_string(), {link});\n        self\n    }}\n",
            ty = key_type(schema, target),
            link = link_expr(target, "value"),
        ),
        FieldValue::Scalar(member) => {
            let ty = scalar_type(schema, model, member);
            let (param, convert) = if STRING_TYPES.contains(&ty.as_str()) {
                (
                    format!("impl Into<{ty}>"),
                    format!("        let value: {ty} = value.into();\n"),
                )
            } else {
                (ty, String::new())
            };
            format!(
                "    pub fn {method}(mut self, value: {param}) -> Self {{\n{convert}        \
                 support::put(&mut self.row, \"{key}\", &value);\n        self\n    }}\n"
            )
        }
    }
}

/// A setter never shadows the builder's own methods.
fn setter_name(field: &str) -> String {
    if BUILDER_METHODS.contains(&field) {
        format!("set_{field}")
    } else {
        field_ident(field)
    }
}

/// Builder methods that are not field setters.
const BUILDER_METHODS: &[&str] = &["insert", "on_shard"];

/// The JSON a relation carries: the target's key under its key field, or a composite key's
/// parts.
pub(super) fn link_expr(target: &RModel, value: &str) -> String {
    if target.is_composite_key() {
        format!("support::link_parts(&{value}.key())")
    } else {
        let key_field = target.pk_field_names().first().copied().unwrap_or("id");
        format!("support::link(\"{key_field}\", &{value}.key())")
    }
}

/// A column's Rust type without its nullability.
fn scalar_type(schema: &CheckedSchema, model: &RModel, member: &RMember) -> String {
    let MemberKind::Scalar {
        ty,
        many,
        enum_name,
        ..
    } = &member.kind
    else {
        return "Json".to_string();
    };
    if !model.is_composite_key() && model.pk_field() == Some(member.name.as_str()) {
        return wrap(&id_type(schema, &model.name), false, *many);
    }
    match enum_name {
        Some(name) => wrap(name, false, *many),
        None => wrap(primitive(*ty), false, *many),
    }
}
