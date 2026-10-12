use super::*;

/// The row interface a fixture insert resolves to: every stored column.
pub(super) fn render_row(schema: &CheckedSchema, model: &RModel) -> String {
    let mut out = format!(
        "\n/** A stored `{0}` row, as a fixture insert returns it. */\nexport interface {0}Row {{\n",
        model.name
    );
    for member in &model.members {
        if matches!(member.kind, MemberKind::Inverse { .. }) {
            continue;
        }
        out.push_str(&format!(
            "  {}: {};\n",
            member.name,
            row_type(schema, model, member)
        ));
    }
    out.push_str("}\n");
    out
}
