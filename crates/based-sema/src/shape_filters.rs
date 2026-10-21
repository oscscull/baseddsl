//! The named-filter projections a shape reaches: its own fields plus every nested body
//! (`field { … }`, `field -> Shape`, `out = edge.far { … }`), each paired with the model
//! it evaluates against.

use based_ast::*;

use crate::ir::*;
use crate::resolve::Cx;

/// One `out = filter(args)` field reached from a shape.
pub(crate) struct FilterUse<'a> {
    pub(crate) out: &'a Ident,
    pub(crate) call: &'a FilterProjection,
    /// Index of the model the filter body resolves against.
    pub(crate) model: usize,
}

/// Every filter projection reachable from `body` (projected from model `model`).
pub(crate) fn shape_filter_uses<'a>(
    body: &'a [ShapeField],
    model: usize,
    cx: &Cx<'a>,
) -> Vec<FilterUse<'a>> {
    let mut out = Vec::new();
    walk(body, model, cx, &mut Vec::new(), &mut out);
    out
}

fn walk<'a>(
    body: &'a [ShapeField],
    model: usize,
    cx: &Cx<'a>,
    stack: &mut Vec<&'a str>,
    out: &mut Vec<FilterUse<'a>>,
) {
    for f in body {
        match f {
            ShapeField::Rename {
                out: name,
                value: ShapeValue::Filter(call),
            } => out.push(FilterUse {
                out: name,
                call,
                model,
            }),
            ShapeField::Nest { field, body } => {
                if let Some(t) = relation_target(model, &field.node, cx) {
                    walk(body, t, cx, stack, out);
                }
            }
            ShapeField::NestRef { field, shape } => {
                let Some(t) = relation_target(model, &field.node, cx) else {
                    continue;
                };
                // A reference cycle is reported by the shape check; stop re-entering it.
                if stack.contains(&shape.node.as_str()) {
                    continue;
                }
                if let Some(body) = cx.shape_bodies.get(&shape.node).copied() {
                    stack.push(shape.node.as_str());
                    walk(body, t, cx, stack, out);
                    stack.pop();
                }
            }
            ShapeField::Flatten { path, body, .. } => {
                let far = path
                    .segments
                    .iter()
                    .try_fold(model, |cur, seg| relation_target(cur, &seg.node, cx));
                if let Some(far) = far {
                    walk(body, far, cx, stack, out);
                }
            }
            ShapeField::Bare(_) | ShapeField::Rename { .. } | ShapeField::Spread { .. } => {}
        }
    }
}

fn relation_target(model: usize, field: &str, cx: &Cx) -> Option<usize> {
    match cx.model(model).member(field).map(|m| &m.kind) {
        Some(MemberKind::Forward { target, .. } | MemberKind::Inverse { target, .. }) => {
            cx.find(target)
        }
        _ => None,
    }
}
