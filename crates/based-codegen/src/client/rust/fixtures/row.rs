use super::*;

/// The whole-model row struct a fixture insert returns, plus its [`RowRef`] impl so a row
/// can stand in for its key.
pub(super) fn render_row(schema: &CheckedSchema, model: &RModel) -> String {
    let name = format!("{}Row", model.name);
    let mut out = format!(
        "\n/// A stored `{}` row, as a fixture insert returns it.\n",
        model.name
    );
    out.push_str(&render_struct(&name, &row_fields(schema, model)));
    if let Some(key) = row_key(model) {
        let ty = key_type(schema, model);
        out.push_str(&format!(
            "impl RowRef<{ty}> for {name} {{\n    fn key(&self) -> {ty} {{\n        {key}\n    }}\n}}\n"
        ));
    }
    out
}

/// The model's stored fields; an opaque raw column reads back as untyped JSON.
fn row_fields(schema: &CheckedSchema, model: &RModel) -> Vec<(String, String)> {
    model_fields(schema, model)
        .into_iter()
        .map(|(name, ty)| match model.member(&name).map(|m| &m.kind) {
            Some(MemberKind::Scalar {
                raw_type: Some(_),
                optional,
                ..
            }) => {
                let ty = wrap("Json", *optional, false);
                (name, ty)
            }
            _ => (name, ty),
        })
        .collect()
}

/// The expression reading a row's key, when the row carries it as the key type: a single
/// scalar key field, or every part of a composite key.
fn row_key(model: &RModel) -> Option<String> {
    let parts = model.pk_field_names();
    if model.is_composite_key() {
        let fields: Vec<String> = parts
            .iter()
            .map(|p| format!("{0}: self.{0}.clone()", field_ident(p)))
            .collect();
        return Some(format!("{}Id {{ {} }}", model.name, fields.join(", ")));
    }
    let field = parts.first()?;
    matches!(
        model.member(field).map(|m| &m.kind),
        Some(MemberKind::Scalar { .. })
    )
    .then(|| format!("self.{}.clone()", field_ident(field)))
}
