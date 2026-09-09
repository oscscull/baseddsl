use super::*;

/// The engine can't inject a tombstone filter into SQL it didn't build. Lint the target
/// model, plus any other soft-delete model whose table the raw text mentions (the
/// joined-table case).
pub(super) fn check_raw_soft_delete_gap(raw: &RawSql, ti: usize, cx: &Cx, sink: &mut Sink) {
    let text: String = raw
        .parts
        .iter()
        .filter_map(|p| match p {
            RawPart::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    for (mi, m) in cx.models.iter().enumerate() {
        let Some(sd) = &m.soft_delete else { continue };
        if mi == ti || mentions_table(&text, &m.table) {
            sink.warn(
                code::RAW_SOFT_DELETE_GAP,
                raw.span,
                format!(
                    "raw SQL on soft-delete model `{}`: engine can't verify the `{}` tombstone filter — confirm it",
                    m.name, sd.field
                ),
            );
        }
    }
}

/// Whether raw SQL text contains `table` as a standalone word (identifier-boundary
/// match, so `user` does not match inside `user_event`).
fn mentions_table(text: &str, table: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(i) = text[from..].find(table) {
        let start = from + i;
        let end = start + table.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}
