//! Dialect-gated construct checks — run at `check` time, once the manifest's compile
//! target is known (the caller passes it in, like [`based_sema::check_target`]). These
//! catch a construct that parses and type-checks but has no equivalent on the target, so
//! codegen would emit invalid SQL: rather than fail silently at runtime, `check` errors.
//!
//! Currently one check: an **ordered to-many nest / m2m flatten on MySQL**. A to-many
//! nest's array order lowers to `ORDER BY` *inside* the JSON aggregate
//! ([`Dialect::json_array_agg`]). MariaDB, Postgres, and SQLite (≥ 3.44) all accept the
//! ordered aggregate; MySQL's `JSON_ARRAYAGG` has no `ORDER BY` clause at all (a syntax
//! error), so an ordered nested array cannot be generated for that target.

use based_ast::{Decl, Path, Shape, ShapeField, Span};
use based_diagnostics::Diagnostic;
use based_sema::{code, CheckedSchema, MemberKind, RModel};
use std::collections::HashMap;

use crate::Dialect;

/// Diagnose every ordered to-many nest / m2m flatten that the `dialect` cannot generate.
/// Empty on every target except MySQL. `decls` is the spread-expanded declaration set
/// (the shape bodies codegen lowers); `schema` is the checked IR.
pub fn ordered_nest_diagnostics(
    schema: &CheckedSchema,
    decls: &[Decl],
    dialect: Dialect,
) -> Vec<Diagnostic> {
    if dialect != Dialect::MySql {
        return Vec::new();
    }
    let shapes: HashMap<&str, &Shape> = decls
        .iter()
        .filter_map(|d| match d {
            Decl::Shape(s) => Some((s.name.node.as_str(), s)),
            _ => None,
        })
        .collect();
    let mut walk = Walk {
        schema,
        shapes,
        diags: Vec::new(),
    };
    for d in decls {
        if let Decl::Shape(s) = d {
            if let Some(model) = schema.model(&s.from.node) {
                let mut stack = vec![s.name.node.as_str()];
                walk.body(&s.body, model, &mut stack);
            }
        }
    }
    walk.diags
}

struct Walk<'a> {
    schema: &'a CheckedSchema,
    shapes: HashMap<&'a str, &'a Shape>,
    diags: Vec<Diagnostic>,
}

/// The traversal an edge realizes, resolved against the checked schema. Mirrors codegen's
/// nest lowering: a to-many inverse aggregates (ordered when its cascade has a sort), a
/// to-one relation nests a single sub-object (never ordered).
enum Edge<'a> {
    ToMany { child: &'a RModel, ordered: bool },
    ToOne { child: &'a RModel },
    None,
}

impl<'a> Walk<'a> {
    fn body(&mut self, body: &'a [ShapeField], model: &'a RModel, stack: &mut Vec<&'a str>) {
        for f in body {
            match f {
                ShapeField::Nest { field, body } => match self.edge(model, &field.node) {
                    Edge::ToMany { child, ordered } => {
                        if ordered {
                            self.report(field.span, &field.node);
                        }
                        self.body(body, child, stack);
                    }
                    Edge::ToOne { child } => self.body(body, child, stack),
                    Edge::None => {}
                },
                ShapeField::NestRef { field, shape } => {
                    let (child, ordered) = match self.edge(model, &field.node) {
                        Edge::ToMany { child, ordered } => (Some(child), ordered),
                        Edge::ToOne { child } => (Some(child), false),
                        Edge::None => (None, false),
                    };
                    if ordered {
                        self.report(field.span, &field.node);
                    }
                    if let (Some(child), Some(sh)) =
                        (child, self.shapes.get(shape.node.as_str()).copied())
                    {
                        // Cycle guard: a shape referenced through a to-many nest can recur
                        // (a comment tree); expand each shape at most once per path.
                        if !stack.contains(&sh.name.node.as_str()) {
                            stack.push(&sh.name.node);
                            self.body(&sh.body, child, stack);
                            stack.pop();
                        }
                    }
                }
                ShapeField::Flatten { out, path, body } => {
                    if let Some(far) = self.far_model(model, path) {
                        // The m2m far-side aggregate orders by the far model's own `@sort`.
                        if !far.sort.is_empty() {
                            self.report(out.span, &out.node);
                        }
                        self.body(body, far, stack);
                    }
                }
                _ => {}
            }
        }
    }

    fn edge(&self, model: &'a RModel, field: &str) -> Edge<'a> {
        let Some(member) = model.member(field) else {
            return Edge::None;
        };
        match &member.kind {
            MemberKind::Inverse { target, via } => {
                let Some(child) = self.schema.model(target) else {
                    return Edge::None;
                };
                if child.is_unique(via) {
                    Edge::ToOne { child } // to-one inverse (has-one)
                } else {
                    // Sort cascade for the traversal: edge `@sort` beats the child model's.
                    let ordered = !member.sort.is_empty() || !child.sort.is_empty();
                    Edge::ToMany { child, ordered }
                }
            }
            MemberKind::Forward { target, .. } => match self.schema.model(target) {
                Some(child) => Edge::ToOne { child },
                None => Edge::None,
            },
            MemberKind::Scalar { .. } => Edge::None,
        }
    }

    /// The far-side model a flatten path resolves to: `segs[0]` is a to-many inverse into
    /// the junction, the rest are forward hops to the far model. `None` on a malformed
    /// path (sema already reports it via the E030x flatten checks).
    fn far_model(&self, root: &'a RModel, path: &Path) -> Option<&'a RModel> {
        let segs = &path.segments;
        let first = root.member(&segs[0].node)?;
        let (junction, via) = match &first.kind {
            MemberKind::Inverse { target, via } => (self.schema.model(target)?, via),
            _ => return None,
        };
        if junction.is_unique(via) {
            return None; // not a to-many first hop
        }
        let mut cur = junction;
        for seg in &segs[1..] {
            match &cur.member(&seg.node)?.kind {
                MemberKind::Forward { target, .. } => cur = self.schema.model(target)?,
                _ => return None,
            }
        }
        Some(cur)
    }

    fn report(&mut self, span: Span, field: &str) {
        self.diags.push(
            Diagnostic::error(
                code::NEST_ORDER_UNSUPPORTED,
                format!(
                    "ordered to-many nested read `{field}` can't be lowered for the MySQL compile target: \
                     a to-many nest's sort order becomes an `ORDER BY` inside `JSON_ARRAYAGG`, and MySQL's \
                     `JSON_ARRAYAGG` has no `ORDER BY` clause (MariaDB, Postgres, and SQLite ≥ 3.44 do), \
                     so there is no valid SQL for this ordered array on MySQL"
                ),
            )
            .at(span)
            .note(
                "compile this project for MariaDB or Postgres, or remove the sort on this nest — the \
                 relation `@sort` on the edge, or the target model's `@sort` — to return the nested \
                 array unordered",
            ),
        );
    }
}
