//! Check a step's `chunk` modifier: only on `update` / `delete`, walking a key that is unique
//! or indexed, not null, and not rewritten by the step itself.

use based_ast::{Chunk, DataOp, DataStep, Ident};

use super::super::table::TableView;
use super::Ck;

impl Ck<'_> {
    pub(crate) fn chunk(&mut self, step: &DataStep, t: &TableView) {
        let Some(chunk) = &step.chunk else { return };
        if matches!(step.op, DataOp::Create { .. }) {
            self.err(
                chunk.span,
                "`chunk` applies to `update` / `delete` — a `create` writes its rows at once",
            );
            return;
        }
        let walk = match &chunk.by {
            Some(by) => {
                self.walk_by(by, t);
                vec![by.node.clone()]
            }
            None => self.walk_pk(chunk, t),
        };
        let DataOp::Update { assigns, .. } = &step.op else {
            return;
        };
        for a in assigns.iter().filter(|a| walk.contains(&a.col.node)) {
            self.err(
                a.col.span,
                format!(
                    "a chunked update can't assign `{}`, the column it walks by",
                    a.col.node
                ),
            );
        }
    }

    /// `by <col>`: an existing, not-null, unique or indexed column.
    fn walk_by(&mut self, by: &Ident, t: &TableView) {
        let problem = match t.col(&by.node) {
            None => format!("table `{}` has no column `{}`", t.name(), by.node),
            Some(c) if c.nullable => format!("`chunk … by {}` needs a not-null column", by.node),
            Some(_) if !t.is_walkable(&by.node) => format!(
                "`chunk … by {}` needs a unique or indexed column — add an index or walk the \
                 primary key",
                by.node
            ),
            Some(_) => return,
        };
        self.err(by.span, problem);
    }

    /// The primary key, which a keyless table lacks.
    fn walk_pk(&mut self, chunk: &Chunk, t: &TableView) -> Vec<String> {
        let pk = t.pk();
        if pk.is_empty() {
            self.err(
                chunk.span,
                format!(
                    "table `{}` has no primary key — name a unique or indexed column with \
                     `chunk N by <col>`",
                    t.name()
                ),
            );
        }
        pk
    }
}
