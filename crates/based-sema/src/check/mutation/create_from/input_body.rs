use super::*;

/// Walk an input shape's fields against the create target, enforcing the presence-driven
/// input rules: every named scalar maps to a settable column; relations are inline
/// nested key blocks (FK link); no computed/aggregate/raw/reach fields; every required,
/// non-engine-managed column is covered; and warn on a named engine-managed column.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_input_shape(
    shape: &str,
    mi: usize,
    cf: &CreateFrom,
    scoped: Option<&Scoped>,
    unscoped: bool,
    has_conflict: bool,
    cx: &Cx,
    sink: &mut Sink,
) {
    let Some(body) = cx.shape_bodies.get(shape).copied() else {
        return;
    };
    let mut seen = vec![shape.to_string()];
    check_input_body(
        body,
        mi,
        None,
        cf,
        scoped,
        unscoped,
        has_conflict,
        &mut seen,
        cx,
        sink,
    );
}

/// Recursively validate an input shape's body against a create target. `exempt` names a
/// back-reference relation the body must not cover (a to-many child's FK to its parent,
/// engine-injected from the parent's created id); `seen` guards a NestRef cycle.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_input_body(
    body: &[ShapeField],
    mi: usize,
    exempt: Option<&str>,
    cf: &CreateFrom,
    scoped: Option<&Scoped>,
    unscoped: bool,
    has_conflict: bool,
    seen: &mut Vec<String>,
    cx: &Cx,
    sink: &mut Sink,
) {
    let m = cx.model(mi);
    let scope_cols: Vec<String> = crate::scope::resolve_inject(scoped, unscoped, &[mi], cx)
        .into_iter()
        .flat_map(|si| si.terms)
        .map(|(f, _)| f)
        .collect();
    let mut covered: Vec<String> = Vec::new();
    // A to-many child's back-reference to its parent is engine-injected, so the child
    // shape neither names nor must cover it.
    if let Some(e) = exempt {
        covered.push(e.to_string());
    }
    for field in body {
        match field {
            ShapeField::Bare(id) => {
                check_input_scalar(m, &id.node, id.span, &scope_cols, &mut covered, sink);
            }
            ShapeField::Rename { out, value } => match value {
                // A rename of one *local* column is a settable mapping (json key `out` →
                // that column). A cross-relation reach, raw, aggregate, or computed value
                // has no single column to write.
                ShapeValue::Path(p) if p.segments.len() == 1 => {
                    check_input_scalar(m, &p.segments[0].node, out.span, &scope_cols, &mut covered, sink);
                }
                ShapeValue::Path(_) => sink.error_note(
                    code::INPUT_BAD_RELATION,
                    out.span,
                    format!("input field `{}` reaches across a relation", out.node),
                    "a `create … from` shape writes local columns; link a relation with an inline `rel { key }` block",
                ),
                _ => sink.error_note(
                    code::INPUT_COMPUTED_FIELD,
                    out.span,
                    format!("input field `{}` is computed / aggregated / raw", out.node),
                    "a `create … from` shape may only name settable columns and FK-link blocks",
                ),
            },
            ShapeField::Nest { field, body } => check_input_nest(
                m, field, body, None, cf, scoped, unscoped, has_conflict, &mut covered, seen, cx, sink,
            ),
            ShapeField::NestRef { field, shape } => {
                match cx.shape_bodies.get(&shape.node).copied() {
                    Some(nbody) => check_input_nest(
                        m, field, nbody, Some(&shape.node), cf, scoped, unscoped, has_conflict,
                        &mut covered, seen, cx, sink,
                    ),
                    None => sink.error(
                        code::INPUT_BAD_RELATION,
                        shape.span,
                        format!("`{}` is not a shape", shape.node),
                    ),
                }
            }
            ShapeField::Flatten { out, .. } => sink.error_note(
                code::INPUT_BAD_RELATION,
                out.span,
                format!("input field `{}` flattens a many-to-many path", out.node),
                "a create writes one row's columns — a flatten is not settable",
            ),
            // Expanded away before sema (based_sema::expand_spreads).
            ShapeField::Spread { .. } => {}
        }
    }
    check_input_coverage(m, cf, &scope_cols, &covered, sink);
}
