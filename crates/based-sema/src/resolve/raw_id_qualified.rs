use super::*;

/// Reject a qualifier in front of `{id}` (`{table}.{id}`, `t.{id}`): `{id}` already
/// expands to the table-qualified key column, so the prefix would qualify it twice and
/// the SQL fails only at runtime. `table` names the row's table when it is known.
pub(crate) fn check_raw_id_qualified(raw: &RawSql, table: Option<&str>, sink: &mut Sink) {
    let mut prev_text: Option<&str> = None;
    for part in &raw.parts {
        match part {
            RawPart::Engine(id) if id.node == "id" => {
                if prev_text.is_some_and(|t| t.trim_end().ends_with('.')) {
                    let table = table.unwrap_or("<table>");
                    sink.error_note(
                        code::RAW_ID_QUALIFIED,
                        id.span,
                        "`{id}` is already qualified; a `.` prefix qualifies it twice",
                        format!("use `{{id}}` alone; it expands to the row's qualified key, e.g. `{table}.id`"),
                    );
                }
                prev_text = None;
            }
            RawPart::Text(t) => prev_text = Some(t),
            _ => prev_text = None,
        }
    }
}
