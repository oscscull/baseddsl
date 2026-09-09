use super::*;

/// Required-column coverage for an input shape: every column that is required (NOT NULL, no
/// default) and not engine-managed must be named by the shape, else the create can't run.
pub(super) fn check_input_coverage(
    m: &RModel,
    cf: &CreateFrom,
    scope_cols: &[String],
    covered: &[String],
    sink: &mut Sink,
) {
    let serial_key_field = m.serial_key_member().map(|mem| mem.name.clone());
    let managed = |name: &str| {
        name == "id"
            || m.created.as_deref() == Some(name)
            || m.updated.as_deref() == Some(name)
            || m.soft_delete.as_ref().map(|s| s.field.as_str()) == Some(name)
            || scope_cols.iter().any(|f| f == name)
            || serial_key_field.as_deref() == Some(name)
    };
    let missing: Vec<&str> = m
        .members
        .iter()
        .filter(|mem| is_required(&mem.kind) && mem.kind.opaque().is_none())
        .map(|mem| mem.name.as_str())
        .filter(|name| !managed(name) && !covered.iter().any(|c| c == name))
        .collect();
    if !missing.is_empty() {
        sink.error_note(
            code::INPUT_MISSING_REQUIRED,
            cf.span,
            format!(
                "input shape for `create {}` is missing required column{}: {}",
                m.name,
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ),
            "every required, non-engine-managed column must be named by the input shape",
        );
    }
}
