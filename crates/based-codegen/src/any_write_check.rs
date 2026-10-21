//! A MySQL/MariaDB check for `rel.any(…)` (or a filter path through a to-many relation,
//! which lowers the same way) in a write: those dialects can't read the table an
//! UPDATE/DELETE writes in a subquery. A key-linked edge is read through a materialized
//! derived table instead, but a custom `on:` edge has no key to project, so it has no valid
//! SQL there and `check` errors rather than fail at runtime.

use based_ast::{Decl, NamedFilter, Path, Predicate, WriteStmt};
use based_diagnostics::Diagnostic;
use based_sema::{code, CheckedSchema, MemberKind, RModel};

use crate::Dialect;

/// Diagnose every self-referential custom-`on:` `.any` in a write that `dialect` can't run.
/// Empty except on the MySQL family.
pub fn any_write_diagnostics(
    schema: &CheckedSchema,
    decls: &[Decl],
    dialect: Dialect,
) -> Vec<Diagnostic> {
    if !dialect.is_mysql_family() {
        return Vec::new();
    }
    let mut walk = Walk {
        schema,
        decls,
        dialect,
        diags: Vec::new(),
    };
    for d in decls {
        if let Decl::Mutation(m) = d {
            walk.writes(&m.body);
        }
    }
    walk.diags
}

struct Walk<'a> {
    schema: &'a CheckedSchema,
    decls: &'a [Decl],
    dialect: Dialect,
    diags: Vec<Diagnostic>,
}

impl<'a> Walk<'a> {
    fn writes(&mut self, body: &'a [WriteStmt]) {
        for w in body {
            let (model, where_) =
                match w {
                    WriteStmt::Update { model, where_, .. }
                    | WriteStmt::Restore { model, where_ } => (model, Some(where_)),
                    WriteStmt::Delete { model, where_ }
                    | WriteStmt::HardDelete { model, where_ } => (model, where_.as_ref()),
                    WriteStmt::Tx(inner) => {
                        self.writes(inner);
                        continue;
                    }
                    WriteStmt::Create { .. } | WriteStmt::Raw(_) => continue,
                };
            if let (Some(m), Some(p)) = (self.schema.model(&model.node), where_) {
                self.pred(p, m, &m.table, &mut Vec::new());
            }
        }
    }

    /// Walk `p` over `model`; `written` is the table the statement writes, `in_filters` the
    /// named filters mid-expansion (a self-referential filter terminates).
    fn pred(
        &mut self,
        p: &'a Predicate,
        model: &'a RModel,
        written: &str,
        in_filters: &mut Vec<&'a str>,
    ) {
        match p {
            Predicate::And(a, b) | Predicate::Or(a, b) => {
                self.pred(a, model, written, in_filters);
                self.pred(b, model, written, in_filters);
            }
            Predicate::Not(inner) => self.pred(inner, model, written, in_filters),
            Predicate::Any { path, pred, .. } => {
                if let Some(child) = self.any_edge(path, model, written) {
                    if let Some(inner) = pred {
                        self.pred(inner, child, written, in_filters);
                    }
                }
            }
            Predicate::FilterCall { name, .. } => {
                self.filter(&name.node, model, written, in_filters);
            }
            Predicate::Bare(path) if path.segments.len() == 1 => {
                self.filter(&path.segments[0].node, model, written, in_filters);
            }
            Predicate::Cmp { path, .. }
            | Predicate::InList { path, .. }
            | Predicate::Bare(path) => {
                self.leaf(path, model, written);
            }
            Predicate::Raw(_) => {}
        }
    }

    /// Check each to-many hop a leaf path steps through: it lowers as an implicit `.any`.
    fn leaf(&mut self, path: &Path, model: &'a RModel, written: &str) {
        let Some((_, hops)) = path.segments.split_last() else {
            return;
        };
        let mut cur = model;
        for seg in hops {
            let Some(mem) = cur.member(&seg.node) else {
                return;
            };
            let Some(target) = mem.kind.target().and_then(|t| self.schema.model(t)) else {
                return;
            };
            if matches!(&mem.kind, MemberKind::Inverse { via, .. } if !target.is_unique(via)) {
                let hop = Path {
                    segments: vec![seg.clone()],
                };
                self.any_edge(&hop, cur, written);
            }
            cur = target;
        }
    }

    fn filter(
        &mut self,
        name: &str,
        model: &'a RModel,
        written: &str,
        in_filters: &mut Vec<&'a str>,
    ) {
        let Some(f) = self.decls.iter().find_map(|d| match d {
            Decl::Filter(f) if f.name.node == name => Some(f),
            _ => None,
        }) else {
            return;
        };
        let f: &'a NamedFilter = f;
        if in_filters.contains(&f.name.node.as_str()) {
            return;
        }
        in_filters.push(&f.name.node);
        self.pred(&f.pred, model, written, in_filters);
        in_filters.pop();
    }

    /// The model `path` ranges over, reporting a custom-`on:` edge into the written table.
    fn any_edge(&mut self, path: &Path, model: &'a RModel, written: &str) -> Option<&'a RModel> {
        let (last, hops) = path.segments.split_last()?;
        let mut cur = model;
        for seg in hops {
            cur = self.schema.model(cur.member(&seg.node)?.kind.target()?)?;
        }
        let MemberKind::Inverse { target, via } = &cur.member(&last.node)?.kind else {
            return None;
        };
        let child = self.schema.model(target)?;
        let custom_on = matches!(
            child.member(via).map(|m| &m.kind),
            Some(MemberKind::Forward {
                custom_on: Some(_),
                ..
            })
        );
        if custom_on && child.table == written {
            self.diags.push(
                Diagnostic::error(
                    code::ANY_SELF_WRITE_UNSUPPORTED,
                    format!(
                        "`{}.any(…)` reads `{written}`, the table this write changes, through a \
                         custom `on:` relation — {} can't read the written table in a subquery, \
                         and a custom join has no key to read it through a derived table",
                        last.node,
                        self.dialect.name()
                    ),
                )
                .at(last.span)
                .note(
                    "use a key-linked relation for this edge, or write the statement as a \
                     `raw(…)` write for this dialect",
                ),
            );
        }
        Some(child)
    }
}
