use super::*;

/// Validate a model's `read|delete|restore: raw`…`` member lines and record them on its
/// `@soft_delete`, so codegen emits them in place of the default soft-delete SQL.
pub(crate) fn validate_soft_overrides(
    ast: &Model,
    mi: usize,
    models: &mut [RModel],
    sink: &mut Sink,
) {
    let declared = ast.decorators.iter().any(|d| d.name.node == "soft_delete");
    let mut overrides = SoftOverrides::default();
    for mem in &ast.members {
        let Member::SoftOverride(ov) = mem else {
            continue;
        };
        let name = op_name(ov.op);
        if !declared {
            sink.error_note(
                code::SOFT_OVERRIDE,
                ov.raw.span,
                format!(
                    "`{name}:` overrides soft-delete SQL, but `{}` has no `@soft_delete`",
                    models[mi].name
                ),
                "add `@soft_delete(<field>)` to the model, or drop the override",
            );
            continue;
        }
        let slot = match ov.op {
            SoftOp::Read => &mut overrides.read,
            SoftOp::Delete => &mut overrides.delete,
            SoftOp::Restore => &mut overrides.restore,
        };
        if slot.is_some() {
            sink.error(
                code::SOFT_OVERRIDE,
                ov.raw.span,
                format!("a second `{name}:` override on `{}`", models[mi].name),
            );
            continue;
        }
        if check_override_body(ov, sink) {
            resolve::check_raw_id(&ov.raw, Some(&models[mi]), sink);
            *slot = Some(ov.raw.clone());
        }
    }
    if let Some(sd) = models[mi].soft_delete.as_mut() {
        sd.overrides = overrides;
    }
}

fn op_name(op: SoftOp) -> &'static str {
    match op {
        SoftOp::Read => "read",
        SoftOp::Delete => "delete",
        SoftOp::Restore => "restore",
    }
}

/// The body must be the fragment codegen splices: a non-empty, param-free predicate
/// (`read`) or `SET` assignment list (`delete`/`restore`), not a whole statement.
/// Returns whether it is usable.
fn check_override_body(ov: &SoftOverride, sink: &mut Sink) -> bool {
    let mut ok = true;
    if ov.raw.parts.iter().any(|p| matches!(p, RawPart::Param(_))) {
        sink.error_note(
            code::SOFT_OVERRIDE,
            ov.raw.span,
            "a soft-delete override takes no `${param}`",
            "it applies to every read or write of the model, which share no params",
        );
        ok = false;
    }
    let blank = ov
        .raw
        .parts
        .iter()
        .all(|p| matches!(p, RawPart::Text(t) if t.trim().is_empty()));
    if blank {
        sink.error(
            code::SOFT_OVERRIDE,
            ov.raw.span,
            "an empty soft-delete override",
        );
        return false;
    }
    let lead = leading_text(&ov.raw);
    let (statement, expected) = match ov.op {
        SoftOp::Read => (
            starts_with_word(&lead, "select") || starts_with_word(&lead, "where"),
            "the live-row predicate alone, e.g. `deleted_at IS NULL AND NOT on_hold`",
        ),
        SoftOp::Delete | SoftOp::Restore => (
            starts_with_word(&lead, "update") || starts_with_word(&lead, "set"),
            "only the SET assignments, e.g. `deleted_at = CURRENT_TIMESTAMP, status = 'void'`",
        ),
    };
    if statement {
        sink.error_note(
            code::SOFT_OVERRIDE,
            ov.raw.span,
            format!("`{}:` takes a fragment, not a statement", op_name(ov.op)),
            format!("write {expected}; the engine keeps the statement, `where`, and scope"),
        );
        ok = false;
    }
    ok
}

/// The override's leading literal text, trimmed and lowercased.
fn leading_text(raw: &RawSql) -> String {
    match raw.parts.first() {
        Some(RawPart::Text(t)) => t.trim_start().to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word).is_some_and(|rest| {
        rest.is_empty() || !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
    })
}
