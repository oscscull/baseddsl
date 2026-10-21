use super::*;

/// Reject `{id}` against a composite-key model: it expands to one key column, and a
/// composite key has none to pick.
pub(crate) fn check_raw_id_composite(raw: &RawSql, model: &RModel, sink: &mut Sink) {
    if !model.is_composite_key() {
        return;
    }
    let cols = model.pk_columns();
    let listed = cols
        .iter()
        .map(|c| format!("`{c}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let spelled = cols
        .iter()
        .map(|c| format!("`{}.{c}`", model.table))
        .collect::<Vec<_>>()
        .join(", ");
    for part in &raw.parts {
        match part {
            RawPart::Engine(id) if id.node == "id" => sink.error_note(
                code::RAW_ID_COMPOSITE,
                id.span,
                format!(
                    "`{{id}}` has no single column to expand to: `{}` has a composite key ({listed})",
                    model.name
                ),
                format!("write the key columns explicitly, e.g. {spelled}"),
            ),
            _ => {}
        }
    }
}
