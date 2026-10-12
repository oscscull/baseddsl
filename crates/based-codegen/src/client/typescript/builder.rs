use super::*;

/// One model's builder class and its constructor function.
pub(super) fn render_builder(schema: &CheckedSchema, fixture: &FixtureModel<'_>) -> String {
    let model = &fixture.model.name;
    let class = format!("{model}Fixture");
    let bools: Vec<String> = fixture
        .model
        .members
        .iter()
        .filter(|m| is_bool(m))
        .map(|m| format!("{:?}", m.name))
        .collect();
    let mut out = format!(
        "\n/** One `{model}` fixture row being built; `{ctor}` starts one. */\nexport class {class} {{\n  \
         private shard = \"\";\n\n  \
         constructor(private readonly row: Record<string, unknown>) {{}}\n",
        ctor = constructor_name(model),
    );
    for field in &fixture.fields {
        out.push_str(&render_setter(schema, fixture.model, field));
    }
    out.push_str(&format!(
        "\n  /** Insert on the shard this key routes to (default: the empty key). */\n  \
         on_shard(key: string): this {{\n    this.shard = key;\n    return this;\n  }}\n\n  \
         /** Insert the row and resolve to it as stored. */\n  \
         insert(connection: Connection): Promise<{model}Row> {{\n    \
         return connection.insert({model:?}, this.row, [{}], this.shard) as Promise<{model}Row>;\n  }}\n}}\n",
        bools.join(", ")
    ));
    out.push_str(&render_constructor(fixture, &class));
    out
}

fn render_setter(schema: &CheckedSchema, model: &RModel, field: &FixtureField<'_>) -> String {
    let (param, value) = match &field.value {
        FieldValue::Link { target, .. } => {
            (format!("{}Ref", target.name), link_expr(target, "value"))
        }
        FieldValue::Scalar(member) => (value_type(schema, model, member), "value".to_string()),
    };
    format!(
        "\n  {}(value: {param}): this {{\n    this.row[{:?}] = {value};\n    return this;\n  }}\n",
        setter_name(field.name),
        field.name,
    )
}

/// `export function order(org: OrgRef, …): OrderFixture`.
fn render_constructor(fixture: &FixtureModel<'_>, class: &str) -> String {
    let model = &fixture.model.name;
    let mut params = Vec::new();
    let mut fills = Vec::new();
    for field in &fixture.fields {
        let value = match (&field.role, &field.value) {
            (Role::Argument, FieldValue::Link { target, .. }) => {
                let arg = binding(field.name);
                params.push(format!("{arg}: {}Ref", target.name));
                link_expr(target, &arg)
            }
            (Role::Placeholder(placeholder), _) => placeholder_expr(field.name, placeholder),
            _ => continue,
        };
        fills.push(format!("    {:?}: {value},\n", field.name));
    }
    let n = if fixture
        .fields
        .iter()
        .any(|f| matches!(&f.role, Role::Placeholder(p) if p.uses_n()))
    {
        "  const n = nextN();\n"
    } else {
        ""
    };
    format!(
        "\n/** A new `{model}` fixture. */\nexport function {ctor}({params}): {class} {{\n{n}  return new {class}({{\n{fills}  }});\n}}\n",
        ctor = constructor_name(model),
        params = params.join(", "),
        fills = fills.concat(),
    )
}

/// The JSON a relation carries: the target's key under its key field, or a composite key's
/// parts.
fn link_expr(target: &RModel, value: &str) -> String {
    let parts: Vec<String> = target
        .pk_field_names()
        .iter()
        .map(|p| format!("{p:?}"))
        .collect();
    if target.is_composite_key() {
        format!("linkParts({value}, [{}])", parts.join(", "))
    } else {
        format!("link({value}, {})", parts.join(", "))
    }
}

fn constructor_name(model: &str) -> String {
    binding(&based_sema::snake_case(model))
}
