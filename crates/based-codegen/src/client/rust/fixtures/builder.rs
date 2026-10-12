use super::*;

/// One model's builder: the struct, its constructor, its setters and `insert`.
pub(super) fn render_builder(schema: &CheckedSchema, fixture: &FixtureModel<'_>) -> String {
    let model = &fixture.model.name;
    let builder = format!("{model}Fixture");
    let mut out = format!(
        "\n/// One `{model}` fixture row being built; [`{ctor}`] starts one.\n\
         #[derive(Debug, Clone)]\n#[must_use]\npub struct {builder} {{\n    \
         row: serde_json::Map<String, serde_json::Value>,\n}}\n",
        ctor = constructor_name(model),
    );
    out.push_str(&render_constructor(schema, fixture, &builder));
    // Setters are named after fields, which may look like conversions or operators.
    out.push_str(&format!(
        "\n#[allow(clippy::wrong_self_convention, clippy::should_implement_trait)]\nimpl {builder} {{\n"
    ));
    for field in &fixture.fields {
        out.push_str(&render_setter(schema, fixture.model, field));
    }
    out.push_str(&format!(
        "    /// Insert the row and return it as stored.\n    \
         pub async fn insert<T: FixtureTransport>(self, client: &Client<T>) -> Result<{model}Row, ClientError> {{\n        \
         support::insert(client, \"{model}\", self.row).await\n    }}\n}}\n"
    ));
    out
}

/// `pub fn order(org: impl RowRef<…>, …) -> OrderFixture`: required relations as
/// arguments, placeholders for required columns.
fn render_constructor(schema: &CheckedSchema, fixture: &FixtureModel<'_>, builder: &str) -> String {
    let model = &fixture.model.name;
    let mut params = Vec::new();
    let mut fills = Vec::new();
    for field in &fixture.fields {
        let value = match (&field.role, &field.value) {
            (Role::Argument, FieldValue::Link { target, .. }) => {
                let arg = field_ident(field.name);
                params.push(format!("{arg}: impl RowRef<{}>", key_type(schema, target)));
                link_expr(target, &arg)
            }
            (Role::Placeholder(placeholder), _) => placeholder_expr(field.name, placeholder),
            _ => continue,
        };
        fills.push(format!(
            "    row.insert(\"{}\".to_string(), {value});\n",
            field.name
        ));
    }
    let uses_n = fixture
        .fields
        .iter()
        .any(|f| matches!(f.role, Role::Placeholder(_)));
    let mut body = String::new();
    if uses_n {
        body.push_str("    let n = support::next_n();\n");
    }
    if fills.is_empty() {
        body.push_str("    let row = serde_json::Map::new();\n");
    } else {
        body.push_str("    let mut row = serde_json::Map::new();\n");
        body.extend(fills);
    }
    format!(
        "\n/// A new `{model}` fixture.\n#[allow(clippy::too_many_arguments)]\npub fn {ctor}({params}) -> {builder} {{\n{body}    {builder} {{ row }}\n}}\n",
        ctor = constructor_name(model),
        params = params.join(", "),
    )
}

fn constructor_name(model: &str) -> String {
    field_ident(&based_sema::snake_case(model))
}
