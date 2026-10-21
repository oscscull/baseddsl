use super::*;

/// Check every `{id}` in a raw fragment whose row is `model` (when known): unqualified,
/// and backed by a single key column.
pub(crate) fn check_raw_id(raw: &RawSql, model: Option<&RModel>, sink: &mut Sink) {
    check_raw_id_qualified(raw, model, sink);
    if let Some(m) = model {
        check_raw_id_composite(raw, m, sink);
    }
}
