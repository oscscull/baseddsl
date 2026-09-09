use super::*;

/// An input shape's relation nest. Two meanings, distinguished by its body:
/// - **FK link** — the block names exactly the target's key part(s) (`rel { id }`): set
///   the FK column(s) from the payload, pointing at an existing row.
/// - **Nested write** — the block names non-key payload (`rel { name, email }`): create
///   the related row too, then link it. A to-one forward edge creates the parent's target
///   before the parent; a to-many inverse edge creates the children after the parent.
///
/// `ref_shape` is `Some(shape_name)` when the nest came from a NestRef (for the cycle
/// guard); `None` for an inline block.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_input_nest(
    m: &RModel,
    field: &Ident,
    body: &[ShapeField],
    ref_shape: Option<&str>,
    cf: &CreateFrom,
    scoped: Option<&Scoped>,
    unscoped: bool,
    has_conflict: bool,
    covered: &mut Vec<String>,
    seen: &mut Vec<String>,
    cx: &Cx,
    sink: &mut Sink,
) {
    // Whatever the nest resolves to, treat it as named so coverage doesn't double-report it.
    covered.push(field.node.clone());
    let Some(mem) = m.member(&field.node) else {
        sink.error(
            code::INPUT_FIELD_NOT_COLUMN,
            field.span,
            format!("`{}` is not a column of `{}`", field.node, m.name),
        );
        return;
    };
    match &mem.kind {
        MemberKind::Forward {
            target, custom_on, ..
        } => {
            check_forward_nest(
                field,
                body,
                target,
                custom_on.is_some(),
                ref_shape,
                cf,
                scoped,
                unscoped,
                has_conflict,
                seen,
                cx,
                sink,
            );
        }
        MemberKind::Inverse { target, via } => {
            // A to-many nested write: create the child collection. The child's back-edge to
            // the parent (`via`) is engine-injected (exempt from coverage); it is never a
            // link (a to-many block always creates its children).
            if has_conflict {
                sink.error_note(
                    code::INPUT_NESTED_WRITE,
                    field.span,
                    format!(
                        "nested write in relation `{}` is not supported with `on conflict`",
                        field.node
                    ),
                    "an upsert reads/writes one table; drop `on conflict`",
                );
                return;
            }
            let via = via.clone();
            let Some(ti) = cx.find(target) else { return };
            recurse_input_nest(
                body,
                ti,
                Some(&via),
                ref_shape,
                cf,
                scoped,
                unscoped,
                has_conflict,
                seen,
                cx,
                sink,
            );
        }
        MemberKind::Scalar { .. } => {
            sink.error_note(
                code::INPUT_BAD_RELATION,
                field.span,
                format!("`{}` is a column, not a relation to link", field.node),
                "drop the `{ … }` block — a scalar column is named bare",
            );
        }
    }
}

/// A forward-edge nest: an FK link (`rel { key }`, requiring the whole key) or a to-one
/// nested write (non-key payload → create the target first). A nested write through a
/// custom-`on:` edge or under `on conflict` is unsupported.
#[allow(clippy::too_many_arguments)]
fn check_forward_nest(
    field: &Ident,
    body: &[ShapeField],
    target: &str,
    custom_on: bool,
    ref_shape: Option<&str>,
    cf: &CreateFrom,
    scoped: Option<&Scoped>,
    unscoped: bool,
    has_conflict: bool,
    seen: &mut Vec<String>,
    cx: &Cx,
    sink: &mut Sink,
) {
    let ti = cx.find(target);
    let key_fields: Vec<String> = ti
        .map(|i| {
            cx.model(i)
                .pk_field_names()
                .iter()
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    if is_key_link(body, &key_fields) {
        // FK link: the block names only key parts — require the *whole* key.
        if !key_fields.is_empty() && body.len() != key_fields.len() {
            sink.error_note(
                code::INPUT_BAD_RELATION,
                field.span,
                format!(
                    "input relation `{}` names only part of `{target}`'s key",
                    field.node
                ),
                "an FK link names the target's full key; add payload columns for a nested write",
            );
        }
        return;
    }
    // Nested write (to-one forward): create the related row, then set the FK.
    if custom_on {
        sink.error_note(
            code::INPUT_NESTED_WRITE,
            field.span,
            format!("nested write through custom-join relation `{}` is not supported", field.node),
            "a custom-`on:` edge owns no FK column to link the created row; link an existing row by key",
        );
        return;
    }
    if has_conflict {
        sink.error_note(
            code::INPUT_NESTED_WRITE,
            field.span,
            format!(
                "nested write in relation `{}` is not supported with `on conflict`",
                field.node
            ),
            "an upsert reads/writes one table; drop `on conflict`, or link the related row by key",
        );
        return;
    }
    let Some(ti) = ti else { return };
    recurse_input_nest(
        body,
        ti,
        None,
        ref_shape,
        cf,
        scoped,
        unscoped,
        has_conflict,
        seen,
        cx,
        sink,
    );
}

/// Recurse into a nested-write child body, pushing/popping the NestRef cycle guard.
#[allow(clippy::too_many_arguments)]
fn recurse_input_nest(
    body: &[ShapeField],
    ti: usize,
    exempt: Option<&str>,
    ref_shape: Option<&str>,
    cf: &CreateFrom,
    scoped: Option<&Scoped>,
    unscoped: bool,
    has_conflict: bool,
    seen: &mut Vec<String>,
    cx: &Cx,
    sink: &mut Sink,
) {
    // A NestRef cycle is rejected by the shape-reference-cycle check, so here just stop and
    // avoid re-validating it during the same pass.
    if let Some(rs) = ref_shape {
        if seen.iter().any(|s| s == rs) {
            return;
        }
        seen.push(rs.to_string());
    }
    check_input_body(
        body,
        ti,
        exempt,
        cf,
        scoped,
        unscoped,
        has_conflict,
        seen,
        cx,
        sink,
    );
    if ref_shape.is_some() {
        seen.pop();
    }
}
