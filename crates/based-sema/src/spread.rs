//! Shape composition — expand `...Base` spreads into concrete fields.
//!
//! A `shape` may splice another same-model shape's field list with `...Base` instead of
//! re-listing it. This pass rewrites every spread into the referenced shape's fields
//! (recursively), so sema-check and codegen only ever see a flat, spread-free body. The
//! parser (which produces the spread) and the formatter (which reprints it) are the only
//! other code that observes a `ShapeField::Spread`.

use crate::ir::{code, Sink};
use based_ast::{Decl, Ident, ShapeField};
use based_diagnostics::Diagnostic;
use std::collections::{HashMap, HashSet};

/// Expand every `...Shape` spread in `decls` in place. Returns the diagnostics raised
/// (unknown target, cross-model splice, cycle, duplicate field, misplaced spread). After
/// this runs, no `ShapeField::Spread` remains anywhere in `decls`.
pub fn expand_spreads(decls: &mut [Decl]) -> Vec<Diagnostic> {
    let mut sink = Sink::default();
    // Snapshot each shape's declared model + body up front, so a base can be read while
    // the decl that owns it is being rewritten.
    let originals: HashMap<String, (String, Vec<ShapeField>)> = decls
        .iter()
        .filter_map(|d| match d {
            Decl::Shape(s) => Some((s.name.node.clone(), (s.from.node.clone(), s.body.clone()))),
            _ => None,
        })
        .collect();

    // Fully-expanded bodies, memoized so a base shared by many parents expands — and
    // reports its own errors — exactly once.
    let mut cache: HashMap<String, Vec<ShapeField>> = HashMap::new();

    for d in decls.iter_mut() {
        if let Decl::Shape(s) = d {
            let had_spread = s
                .body
                .iter()
                .any(|f| matches!(f, ShapeField::Spread { .. }));
            let mut stack = Vec::new();
            let expanded =
                expand_named(&s.name.node, &originals, &mut cache, &mut stack, &mut sink);
            // Duplicates only arise from a spread, so only a composed shape is checked —
            // a hand-written shape's field list is left exactly as sema saw it before.
            if had_spread {
                check_duplicates(&s.name.node, &expanded, &mut sink);
            }
            s.body = expanded;
        }
    }
    sink.diags
}

type Originals = HashMap<String, (String, Vec<ShapeField>)>;

/// The fully-expanded body of a named shape (memoized). `stack` is the chain of shapes
/// currently mid-expansion, so a spread closing back onto an ancestor is a cycle error.
fn expand_named(
    name: &str,
    originals: &Originals,
    cache: &mut HashMap<String, Vec<ShapeField>>,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) -> Vec<ShapeField> {
    if let Some(cached) = cache.get(name) {
        return cached.clone();
    }
    let Some((from, body)) = originals.get(name) else {
        return Vec::new();
    };
    stack.push(name.to_string());
    let expanded = expand_body(body, from, originals, cache, stack, sink);
    stack.pop();
    cache.insert(name.to_string(), expanded.clone());
    expanded
}

/// Expand the spreads at one shape-body level. `model` is the model this body projects;
/// a spread's target must share it.
fn expand_body(
    body: &[ShapeField],
    model: &str,
    originals: &Originals,
    cache: &mut HashMap<String, Vec<ShapeField>>,
    stack: &mut Vec<String>,
    sink: &mut Sink,
) -> Vec<ShapeField> {
    let mut out = Vec::with_capacity(body.len());
    for f in body {
        match f {
            ShapeField::Spread { shape } => {
                let Some((from, _)) = originals.get(&shape.node) else {
                    sink.error(
                        code::SHAPE_SPREAD_UNKNOWN,
                        shape.span,
                        format!("`...{}` names no shape", shape.node),
                    );
                    continue;
                };
                if from != model {
                    sink.error_note(
                        code::SHAPE_SPREAD_MODEL,
                        shape.span,
                        format!(
                            "spread shape `{}` is from `{from}`, but this shape is from `{model}`",
                            shape.node
                        ),
                        "a spread splices same-model columns — to embed a related model, nest it (`field -> Shape`)",
                    );
                    continue;
                }
                if stack.iter().any(|s| s == &shape.node) {
                    sink.error(
                        code::SHAPE_SPREAD_CYCLE,
                        shape.span,
                        format!(
                            "shape spread cycle: `{}` transitively spreads itself",
                            shape.node
                        ),
                    );
                    continue;
                }
                out.extend(expand_named(&shape.node, originals, cache, stack, sink));
            }
            ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
                reject_nested_spread(body, sink);
                out.push(f.clone());
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// A spread is top-level only: inside a nest the surrounding model is a *related* one, so
/// same-model splice has no meaning there — embed a shape by name (`field -> Shape`) instead.
fn reject_nested_spread(body: &[ShapeField], sink: &mut Sink) {
    for f in body {
        match f {
            ShapeField::Spread { shape } => sink.error_note(
                code::SHAPE_SPREAD_PLACE,
                shape.span,
                format!("`...{}` can only appear at a shape's top level", shape.node),
                "inside a nest, embed a shape by name (`field -> Shape`), not a same-model spread",
            ),
            ShapeField::Nest { body, .. } | ShapeField::Flatten { body, .. } => {
                reject_nested_spread(body, sink);
            }
            _ => {}
        }
    }
}

/// After expansion, two fields projecting the same name collide (a spread duplicating a
/// local field, or two spreads overlapping). Report the second occurrence.
fn check_duplicates(owner: &str, body: &[ShapeField], sink: &mut Sink) {
    let mut seen: HashSet<&str> = HashSet::new();
    for f in body {
        if let Some(id) = field_out(f) {
            if !seen.insert(id.node.as_str()) {
                sink.error_note(
                    code::SHAPE_SPREAD_DUP,
                    id.span,
                    format!(
                        "shape `{owner}` projects `{}` twice after composition",
                        id.node
                    ),
                    "a spread and a local field (or two spreads) define the same column — drop one",
                );
            }
        }
    }
}

/// The output field name a shape field projects, or `None` for a spread (which projects
/// nothing on its own — it is expanded away).
fn field_out(f: &ShapeField) -> Option<&Ident> {
    match f {
        ShapeField::Bare(id) => Some(id),
        ShapeField::Rename { out, .. } | ShapeField::Flatten { out, .. } => Some(out),
        ShapeField::Nest { field, .. } | ShapeField::NestRef { field, .. } => Some(field),
        ShapeField::Spread { .. } => None,
    }
}
