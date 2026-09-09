use super::*;

/// The columns a `create … from` input shape sets (its bare scalar fields, single-column
/// renames, and FK-link relation names) — the bulk counterpart of a create block's assigns,
/// used to check the conflict target's coverage.
pub(super) fn input_shape_columns(m: &Mutation, cf: &CreateFrom, cx: &Cx) -> Vec<String> {
    let Some(param) = m.params.iter().find(|p| p.name.node == cf.param.node) else {
        return Vec::new();
    };
    let Some(based_ast::BaseType::Model(name)) = param.ty.as_ref().map(|t| &t.base) else {
        return Vec::new();
    };
    let Some(body) = cx.shape_bodies.get(&name.node).copied() else {
        return Vec::new();
    };
    let mut cols = Vec::new();
    for f in body {
        match f {
            ShapeField::Bare(id) => cols.push(id.node.clone()),
            ShapeField::Rename {
                out,
                value: ShapeValue::Path(p),
            } if p.segments.len() == 1 => {
                // The output json key names the settable column (a single-column rename).
                cols.push(out.node.clone());
                cols.push(p.segments[0].node.clone());
            }
            ShapeField::Nest { field, .. } => cols.push(field.node.clone()),
            _ => {}
        }
    }
    cols
}

/// Resolve a `create … from $param`'s input shape: `$param` must be a declared parameter
/// typed as a `shape` (single `create Model from`) or `shape[]` (bulk `create Model[]
/// from`) whose `from` model is exactly the create target. Returns the shape name, or
/// `None` (with a diagnostic) when any of those fails.
pub(super) fn resolve_input_shape(
    m: &Mutation,
    cf: &CreateFrom,
    model: &Ident,
    cx: &Cx,
    sink: &mut Sink,
) -> Option<String> {
    let Some(param) = m.params.iter().find(|p| p.name.node == cf.param.node) else {
        sink.error(
            code::INPUT_PARAM_TYPE,
            cf.span,
            format!(
                "`from ${}` names no parameter of mutation `{}`",
                cf.param.node, m.name.node
            ),
        );
        return None;
    };
    let want = if cf.bulk { "[]" } else { "" };
    let Some(ty) = &param.ty else {
        sink.error_note(
            code::INPUT_PARAM_TYPE,
            cf.span,
            format!("parameter `{}` needs an explicit shape type", cf.param.node),
            format!("type it `{}: {}{want}`", cf.param.node, model.node),
        );
        return None;
    };
    let based_ast::BaseType::Model(name) = &ty.base else {
        sink.error_note(
            code::INPUT_PARAM_TYPE,
            cf.span,
            format!(
                "`create {} from ${}` needs a shape-typed param",
                model.node, cf.param.node
            ),
            format!(
                "type `{}` as a `shape` projecting `{}` (`{}{want}`)",
                cf.param.node, model.node, model.node
            ),
        );
        return None;
    };
    let Some(from_model) = cx.shapes.get(&name.node) else {
        sink.error_note(
            code::INPUT_PARAM_TYPE,
            cf.span,
            format!("`{}` is not a shape", name.node),
            "the row-input type of a `create … from` must be a declared `shape`",
        );
        return None;
    };
    if ty.many != cf.bulk {
        let (spelled, need) = if cf.bulk {
            (name.node.clone(), format!("{}[]", name.node))
        } else {
            (format!("{}[]", name.node), name.node.clone())
        };
        sink.error_note(
            code::INPUT_PARAM_TYPE,
            cf.span,
            format!("`create {}{} from` expects `{need}`, got `{spelled}`", model.node, want),
            if cf.bulk {
                "a bulk `create Model[] from` takes a `shape[]` param"
            } else {
                "a single `create Model from` takes a `shape` param (write `Model[]` for the bulk form)"
            },
        );
        return None;
    }
    if from_model != &model.node {
        sink.error_note(
            code::INPUT_PARAM_TYPE,
            cf.span,
            format!(
                "shape `{}` projects `{from_model}`, but this creates `{}`",
                name.node, model.node
            ),
            "a `create … from` shape must project the model being created (round-trip: read-shape == write-shape)",
        );
        return None;
    }
    Some(name.node.clone())
}
