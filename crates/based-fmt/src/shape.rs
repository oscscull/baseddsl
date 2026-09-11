//! Reprint a shape's fields, inline and block form (bare, rename, nest, flatten,
//! spread), the value a rename maps to, and an aggregate call.

use crate::*;

pub(crate) fn shape_field_inline(f: &ShapeField) -> String {
    match f {
        ShapeField::Bare(id) => id.node.clone(),
        ShapeField::Rename { out, value } => format!("{} = {}", out.node, shape_value(value)),
        ShapeField::Nest { field, body } => format!(
            "{} {{ {} }}",
            field.node,
            body.iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ShapeField::NestRef { field, shape } => format!("{} -> {}", field.node, shape.node),
        ShapeField::Flatten { out, path: p, body } => format!(
            "{} = {} {{ {} }}",
            out.node,
            path(p),
            body.iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ShapeField::Spread { shape } => format!("...{}", shape.node),
    }
}

pub(crate) fn shape_field_block(f: &ShapeField, rename_w: usize) -> String {
    match f {
        ShapeField::Bare(id) => id.node.clone(),
        ShapeField::Rename { out, value } => {
            format!("{:<rename_w$} = {}", out.node, shape_value(value))
        }
        ShapeField::Nest { field, body } => format!(
            "{} {{ {} }}",
            field.node,
            body.iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ShapeField::NestRef { field, shape } => format!("{} -> {}", field.node, shape.node),
        ShapeField::Flatten { out, path: p, body } => format!(
            "{:<rename_w$} = {} {{ {} }}",
            out.node,
            path(p),
            body.iter()
                .map(shape_field_inline)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ShapeField::Spread { shape } => format!("...{}", shape.node),
    }
}

fn shape_value(v: &ShapeValue) -> String {
    match v {
        ShapeValue::Path(p) => path(p),
        ShapeValue::Raw(r) => raw_sql(r),
        ShapeValue::Agg(a) => aggregate(a),
        ShapeValue::Computed(e) => shape_expr(e, 0),
    }
}

fn aggregate(a: &AggCall) -> String {
    match &a.arg {
        Some(p) => format!("{}({})", a.func.node, path(p)),
        None => format!("{}()", a.func.node),
    }
}
