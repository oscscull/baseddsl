//! The JSON output-path list: which projected columns the runtime reassembles into nested
//! JSON, mirroring the projection so a `json` column read back as a string re-parses.

use super::*;

/// Recurse a shape body, appending the output path of every `json` leaf. `prefix` is the
/// accumulated path to this body — empty at the root, `field.` inside a to-one nest,
/// `field[].` inside a to-many nest/flatten (the `[]` marks the array the runtime descends
/// into per element), matching how the runtime reassembles the row.
pub(crate) fn walk_shape_json(
    schema: &CheckedSchema,
    decls: &[Decl],
    body: &[ShapeField],
    model: &RModel,
    prefix: &str,
    out: &mut Vec<String>,
) {
    for f in body {
        match f {
            ShapeField::Bare(id) => {
                if is_json_scalar(model, &id.node) {
                    out.push(format!("{prefix}{}", id.node));
                }
            }
            ShapeField::Rename { out: name, value } => {
                if let ShapeValue::Path(p) = value {
                    if matches!(
                        path_scalar_primitive(schema, Some(model), p),
                        Some(Primitive::Json)
                    ) {
                        out.push(format!("{prefix}{}", name.node));
                    }
                }
            }
            ShapeField::Nest { field, body } => {
                walk_nest_json(schema, decls, &field.node, body, model, prefix, out);
            }
            ShapeField::NestRef { field, shape } => {
                if let Some((target, _)) = relation_target(model, &field.node) {
                    if let Some(sh) = find_shape(decls, &shape.node, target) {
                        walk_nest_json(schema, decls, &field.node, &sh.body, model, prefix, out);
                    }
                }
            }
            ShapeField::Flatten {
                out: name,
                path,
                body,
            } => {
                if let Some(far) = relation_terminal_model(schema, model, path) {
                    let p = format!("{prefix}{}{ARRAY_MARK}{NEST_SEP}", name.node);
                    walk_shape_json(schema, decls, body, far, &p, out);
                }
            }
            ShapeField::Spread { .. } => {}
        }
    }
}

/// Recurse into one relation nest, extending `prefix` with `field.` (to-one) or `field[].`
/// (to-many) before walking the child body against the target model.
fn walk_nest_json(
    schema: &CheckedSchema,
    decls: &[Decl],
    field: &str,
    body: &[ShapeField],
    model: &RModel,
    prefix: &str,
    out: &mut Vec<String>,
) {
    let Some((target, many)) = relation_target(model, field) else {
        return;
    };
    let Some(child) = schema.model(target) else {
        return;
    };
    let p = if many {
        format!("{prefix}{field}{ARRAY_MARK}{NEST_SEP}")
    } else {
        format!("{prefix}{field}{NEST_SEP}")
    };
    walk_shape_json(schema, decls, body, child, &p, out);
}

/// The output field-paths ([`NEST_SEP`]/[`ARRAY_MARK`]-joined, relative to one result row)
/// of every `json`-typed leaf a return projection produces — the runtime's list for
/// normalizing json columns read back as strings into structured JSON. Mirrors
/// [`project_return`]: a named shape walks its body; a bare-model return lists its stored
/// `json` columns. Shared by the read side (queries) and the write side (a mutation's
/// declared-shape re-select), so a written json value round-trips identically.
pub(crate) fn json_output_paths(
    schema: &CheckedSchema,
    decls: &[Decl],
    ret_shape: Option<&str>,
    target: &str,
    root: &RModel,
) -> Vec<String> {
    let mut out = Vec::new();
    match ret_shape {
        Some(name) => {
            if let Some(shape) = find_shape(decls, name, target) {
                walk_shape_json(schema, decls, &shape.body, root, "", &mut out);
            }
        }
        None => {
            for mem in &root.members {
                if is_json_scalar(root, &mem.name) {
                    out.push(mem.name.clone());
                }
            }
        }
    }
    out
}
