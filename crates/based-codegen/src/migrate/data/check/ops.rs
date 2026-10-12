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
        if let DataOp::Update {
            where_: Some(p), ..
        }
        | DataOp::Delete {
            where_: Some(p), ..
        } = &step.op
        {
            self.predicate(p, &t, Scope::ROW);
        }
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
            DataOp::Update { assigns, .. } => self.assigns(assigns, &t, Scope::ROW),
            DataOp::Delete { .. } => {}
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
        let col_set = |row: &DataRow| -> BTreeSet<String> {
            row.assigns.iter().map(|a| a.col.node.clone()).collect()
        };
        let cols = col_set(first);
        for row in rows {
            self.assigns(&row.assigns, t, Scope::LITERALS);
            if col_set(row) != cols {
                self.err(
                    row.span,
                    "every row of a `create … from [ … ]` sets the same columns",
                );
            }
        }
        let missing: Vec<String> = t
            .cols
            .iter()
            .filter(|c| !c.nullable && !c.auto && !cols.contains(&c.name))
            .map(|c| format!("`{}`", c.name))
            .collect();
        if !missing.is_empty() {
            self.err(
                first.span,
                format!(
                    "missing required column(s) {} (not null, no default)",
                    missing.join(", ")
                ),
            );
        }
    }

    fn conflict(&mut self, c: &DataConflict, t: &TableView) {
        let sorted = |mut k: Vec<String>| {
            k.sort();
            k
        };
        let target = sorted(c.target.iter().map(|i| i.node.clone()).collect());
        if !t.unique_keys().into_iter().map(sorted).any(|k| k == target) {
            self.err(
                c.span,
                format!(
                    "`on conflict ({})` is not a unique key of `{}`",
                    target.join(", "),
                    t.name()
                ),
            );
        }
        if let Some(s) = c.spread.as_ref().filter(|s| s.source.node != "incoming") {
            self.err(s.source.span, "the spread is `...incoming`");
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
            match t.col(&a.col.node) {
                None => self.err(
                    a.col.span,
                    format!("table `{}` has no column `{}`", t.name(), a.col.node),
                ),
                Some(col) if col.generated => self.err(
                    a.col.span,
                    format!(
                        "`{}` is a generated column — its value is derived",
                        col.name
                    ),
                ),
                Some(col) => self.fits(col, &ty, a.col.span),
            }
        }
    }
}
