use super::*;

/// A `create` must assign every *required* column — a non-optional, non-defaulted stored
/// column or forward FK — except engine-managed ones: `id`, `@created`/`@updated`, the
/// `@soft_delete` field, and the `@scope` columns the chosen alternative sets on insert.
/// A scope column outside the chosen alternative (or on an `unscoped` create) stays
/// required and surfaces here.
pub(super) fn check_create_required(
    mi: usize,
    assigns: &[Assign],
    at: &Ident,
    scoped: Option<&Scoped>,
    unscoped: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let m = cx.model(mi);
    let assigned: Vec<&str> = assigns.iter().map(|a| a.col.node.as_str()).collect();
    let scope_cols: Vec<(String, String)> =
        crate::scope::resolve_inject(scoped, unscoped, &[mi], cx)
            .into_iter()
            .flat_map(|si| si.terms)
            .collect();
    let serial_key_field = m.serial_key_member().map(|mem| mem.name.clone());
    let managed = |name: &str| {
        name == "id"
            || m.created.as_deref() == Some(name)
            || m.updated.as_deref() == Some(name)
            || m.soft_delete.as_ref().map(|s| s.field.as_str()) == Some(name)
            || scope_cols.iter().any(|(f, _)| f == name)
            // A composite key's `serial` part is DB-generated, like `id`.
            || serial_key_field.as_deref() == Some(name)
    };
    // A required *opaque* column cannot be supplied, so the create is unwritable until the
    // column is made nullable or defaulted.
    let unsuppliable: Vec<&str> = m
        .members
        .iter()
        .filter(|mem| is_required(&mem.kind) && mem.kind.opaque().is_some())
        .map(|mem| mem.name.as_str())
        .filter(|name| !managed(name))
        .collect();
    if !unsuppliable.is_empty() {
        sink.error_note(
            code::OPAQUE_ASSIGN,
            at.span,
            format!(
                "`create {}` cannot supply the opaque column{} {}",
                m.name,
                if unsuppliable.len() == 1 { "" } else { "s" },
                unsuppliable.join(", ")
            ),
            "an opaque `raw(…)` column is excluded from writes — make it nullable (`?`) or give it a `(default …)`",
        );
    }
    let missing: Vec<&str> = m
        .members
        .iter()
        .filter(|mem| is_required(&mem.kind) && mem.kind.opaque().is_none())
        .map(|mem| mem.name.as_str())
        .filter(|name| !managed(name) && !assigned.contains(name))
        .collect();
    if !missing.is_empty() {
        sink.error(
            code::CREATE_MISSING,
            at.span,
            format!(
                "`create {}` is missing required field{}: {}",
                m.name,
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ),
        );
    }
}
