//! Check a data step's `where` (and `case when`) predicate over one table.

use based_ast::{Ident, Op, Path, Predicate, Span, Value};

use super::super::relation::child_link;
use super::super::table::{Col, Fam, TableView};
use super::expr::{fits, Ty};
use super::{Ck, Scope};

impl Ck<'_> {
    pub(crate) fn predicate(&mut self, p: &Predicate, t: &TableView, scope: Scope) {
        match p {
            Predicate::And(a, b) | Predicate::Or(a, b) => {
                self.predicate(a, t, scope);
                self.predicate(b, t, scope);
            }
            Predicate::Not(inner) => self.predicate(inner, t, scope),
            Predicate::Cmp { path, op, value } => self.cmp(path, *op, value, t, scope),
            Predicate::InList { path, values } => {
                let lhs = self.column_path(path, t);
                for v in values {
                    let rhs = self.value(v, t, scope);
                    self.comparable(&lhs, &rhs, path, span_of(path));
                }
            }
            Predicate::Bare(path) => {
                let ty = self.column_path(path, t);
                if !matches!(ty, Ty::Col(Fam::Bool) | Ty::JsonPath | Ty::Unknown) {
                    self.err(
                        span_of(path),
                        format!("`{}` is not a bool column", path_text(path)),
                    );
                }
            }
            Predicate::Any { path, by, pred } => self.any(path, by.as_ref(), pred.as_deref(), t),
            Predicate::FilterCall { name, .. } => self.err(
                name.span,
                "named filters aren't available in a migration — write the condition out",
            ),
            Predicate::Raw(raw) => self.err(
                raw.span,
                "a raw predicate isn't available in a data step — use a raw(<dialect>) line",
            ),
        }
    }

    fn cmp(&mut self, path: &Path, op: Op, value: &Value, t: &TableView, scope: Scope) {
        let lhs = self.column_path(path, t);
        let rhs = self.value(value, t, scope);
        match op {
            Op::Has => self.err(
                span_of(path),
                "`has` isn't available in a data step — use a raw(<dialect>) line",
            ),
            Op::In => self.err(
                span_of(path),
                "`in` takes a value list in a data step: `col in (a, b)`",
            ),
            Op::Like if !matches!(lhs, Ty::Col(Fam::Text) | Ty::JsonPath | Ty::Unknown) => {
                self.err(span_of(path), "`~` (like) needs a text column");
            }
            _ if rhs == Ty::Null && !matches!(op, Op::Eq | Op::Ne) => {
                self.err(span_of(path), "only `=` / `!=` compare against null");
            }
            _ => self.comparable(&lhs, &rhs, path, span_of(path)),
        }
    }

    /// Report a comparison whose operands can't meet.
    fn comparable(&mut self, lhs: &Ty, rhs: &Ty, path: &Path, span: Span) {
        let Ty::Col(fam) = lhs else { return };
        if rhs == &Ty::Null {
            return;
        }
        let as_col = Col {
            name: path_text(path),
            ty: fam.name().to_string(),
            fam: fam.clone(),
            nullable: true,
            has_default: false,
            generated: false,
            auto: false,
        };
        if let Err(msg) = fits(&as_col, rhs) {
            self.err(span, msg.replace("cannot write", "cannot compare"));
        }
    }

    fn any(&mut self, path: &Path, by: Option<&Ident>, pred: Option<&Predicate>, t: &TableView) {
        let [child] = path.segments.as_slice() else {
            self.err(
                span_of(path),
                "`.any` in a data step names one child table: `<table>.any(…)`",
            );
            return;
        };
        match (
            child_link(self.snap, t, &child.node, by.map(|b| b.node.as_str())),
            pred,
        ) {
            (Ok((view, _)), Some(p)) => self.predicate(p, &view, Scope::ROW),
            (Ok(_), None) => {}
            (Err(msg), _) => self.err(child.span, msg),
        }
    }
}

pub(crate) fn span_of(path: &Path) -> Span {
    let first = path.segments[0].span;
    Span {
        end: path.segments.last().map_or(first.end, |s| s.span.end),
        ..first
    }
}

pub(crate) fn path_text(path: &Path) -> String {
    path.segments
        .iter()
        .map(|s| s.node.as_str())
        .collect::<Vec<_>>()
        .join(".")
}
