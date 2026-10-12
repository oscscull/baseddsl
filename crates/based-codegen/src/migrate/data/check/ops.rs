//! Check each data-step form: `create` (rows, required columns, upsert key), `update`, and
//! `delete`.

use std::collections::BTreeSet;

use based_ast::{DataAssign, DataConflict, DataOp, DataRow, DataStep, Span};

use super::super::table::TableView;
use super::{Ck, Scope};

impl Ck<'_> {
    pub(crate) fn step(&mut self, step: &DataStep) {
        let table = step.op.table();
        let snap = self.snap;
        let Some(t) = TableView::find(snap, &table.node) else {
            self.err(
                table.span,
                format!(
                    "unknown table `{}` at this point in the migration",
                    table.node
                ),
            );
            return;
        };
        match &step.op {
            DataOp::Create {
                rows,
                conflict,
                bulk,
                ..
            } => {
                self.create(rows, *bulk, &t, step.span);
                if let Some(c) = conflict {
                    self.conflict(c, &t);
                }
            }
            DataOp::Update {
                where_, assigns, ..
            } => {
                if let Some(p) = where_ {
                    self.predicate(p, &t, Scope::ROW);
                }
                self.assigns(assigns, &t, Scope::ROW);
            }
            DataOp::Delete { where_, .. } => {
                if let Some(p) = where_ {
                    self.predicate(p, &t, Scope::ROW);
                }
            }
        }
        self.chunk(step, &t);
    }

    fn create(&mut self, rows: &[DataRow], bulk: bool, t: &TableView, span: Span) {
        let Some(first) = rows.first() else {
            if bulk {
                self.err(span, "`create … from [ ]` needs at least one row");
            }
            return;
        };
        let cols: BTreeSet<&str> = first.assigns.iter().map(|a| a.col.node.as_str()).collect();
        for row in rows {
            self.assigns(&row.assigns, t, Scope::LITERALS);
            let these: BTreeSet<&str> = row.assigns.iter().map(|a| a.col.node.as_str()).collect();
            if these != cols {
                self.err(
                    row.span,
                    "every row of a `create … from [ … ]` sets the same columns",
                );
            }
        }
        let missing: Vec<&str> = t
            .cols
            .iter()
            .filter(|c| !c.nullable && !c.auto && !cols.contains(c.name.as_str()))
            .map(|c| c.name.as_str())
            .collect();
        if !missing.is_empty() {
            self.err(
                first.span,
                format!(
                    "missing required column(s) {} (not null, no default)",
                    missing
                        .iter()
                        .map(|c| format!("`{c}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }

    fn conflict(&mut self, c: &DataConflict, t: &TableView) {
        let mut target: Vec<String> = c.target.iter().map(|i| i.node.clone()).collect();
        target.sort();
        let is_key = t.unique_keys().into_iter().any(|mut k| {
            k.sort();
            k == target
        });
        if !is_key {
            self.err(
                c.span,
                format!(
                    "`on conflict ({})` is not a unique key of `{}`",
                    target.join(", "),
                    t.name()
                ),
            );
        }
        if let Some(s) = &c.spread {
            if s.source.node != "incoming" {
                self.err(s.source.span, "the spread is `...incoming`");
            }
        }
        self.assigns(
            &c.update,
            t,
            Scope {
                columns: true,
                incoming: true,
            },
        );
    }

    fn assigns(&mut self, assigns: &[DataAssign], t: &TableView, scope: Scope) {
        let mut seen = BTreeSet::new();
        for a in assigns {
            if !seen.insert(a.col.node.as_str()) {
                self.err(a.col.span, format!("`{}` is assigned twice", a.col.node));
            }
            let ty = self.expr(&a.value, t, scope);
            let Some(col) = t.col(&a.col.node) else {
                self.err(
                    a.col.span,
                    format!("table `{}` has no column `{}`", t.name(), a.col.node),
                );
                continue;
            };
            if col.generated {
                self.err(
                    a.col.span,
                    format!(
                        "`{}` is a generated column — its value is derived",
                        col.name
                    ),
                );
                continue;
            }
            self.fits(col, &ty, a.col.span);
        }
    }
}
