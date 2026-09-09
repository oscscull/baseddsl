use super::*;

/// A named scalar field of an input shape: it must resolve to a settable scalar column of
/// the target. Warns when it names an engine-managed column.
pub(super) fn check_input_scalar(
    m: &RModel,
    field: &str,
    span: Span,
    scope_cols: &[String],
    covered: &mut Vec<String>,
    sink: &mut Sink,
) {
    let Some(mem) = m.member(field) else {
        sink.error(
            code::INPUT_FIELD_NOT_COLUMN,
            span,
            format!("`{}` is not a column of `{}`", field, m.name),
        );
        return;
    };
    match &mem.kind {
        MemberKind::Forward { .. } | MemberKind::Inverse { .. } => {
            sink.error_note(
                code::INPUT_BAD_RELATION,
                span,
                format!("input field `{field}` is a relation, not a column"),
                "link a relation with an inline `rel { key }` block (the target's key)",
            );
            // Mark it named so coverage doesn't *also* report it missing (one diagnostic).
            covered.push(field.to_string());
            return;
        }
        MemberKind::Scalar { .. } if mem.kind.opaque().is_some() => {
            sink.error_note(
                code::INPUT_FIELD_NOT_COLUMN,
                span,
                format!("input field `{field}` is an opaque `raw(…)` column"),
                "an opaque column cannot be written — drop it from the input shape",
            );
            return;
        }
        MemberKind::Scalar { .. } => {}
    }
    warn_managed_input(m, field, span, scope_cols, sink);
    covered.push(field.to_string());
}

/// Warn when an input shape names an engine-managed column: an `@created`/`@updated`
/// timestamp (the explicit value overrides the auto-stamp) or a `@scope` column (the value
/// is silently engine-injected from `$ctx`).
fn warn_managed_input(m: &RModel, field: &str, span: Span, scope_cols: &[String], sink: &mut Sink) {
    if scope_cols.iter().any(|c| c == field) {
        sink.warn_note(
            code::INPUT_NAMES_SCOPE,
            span,
            format!("input shape names the `@scope` column `{field}`"),
            "its value is ignored — the engine injects the scope from `$ctx`",
        );
    } else if m.created.as_deref() == Some(field) || m.updated.as_deref() == Some(field) {
        sink.warn_note(
            code::INPUT_NAMES_TIMESTAMP,
            span,
            format!("input shape names the engine-managed timestamp `{field}`"),
            "the payload value is written verbatim, overriding the automatic now() stamp",
        );
    }
}
