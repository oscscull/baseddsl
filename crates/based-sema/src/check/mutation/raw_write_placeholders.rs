use super::*;

/// A raw write statement has no target model, so `{table}` / `{id}` have nothing to
/// expand to; reject them rather than emit an empty identifier.
pub(super) fn check_raw_write_placeholders(raw: &RawSql, sink: &mut Sink) {
    for part in &raw.parts {
        if let RawPart::Engine(id) = part {
            if id.node == "table" || id.node == "id" {
                sink.error_note(
                    code::RAW_WRITE_PLACEHOLDER,
                    id.span,
                    format!("`{{{}}}` has no model to expand to in a raw write", id.node),
                    "spell the table and column names in the SQL",
                );
            }
        }
    }
}
