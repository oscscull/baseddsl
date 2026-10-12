//! Advance a [`Snapshot`] by one structural [`Step`] — the schema state at a given line of an
//! `up.mig`, which data steps are checked against and SQLite rebuilds target.

use super::diff::Step;
use super::model::{Snapshot, TableSnap};

/// Apply `step` to `snap` in place. Steps that emit no schema change (scope notes, `raw`)
/// leave it untouched.
pub(crate) fn evolve(snap: &mut Snapshot, step: &Step) {
    match step {
        Step::CreateTable(t) => {
            snap.tables.retain(|x| x.name != t.name);
            snap.tables.push(t.clone());
            snap.tables.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::DropTable { name, .. } => snap.tables.retain(|t| &t.name != name),
        Step::RenameTable { from, to, .. } => {
            if let Some(t) = table_mut(snap, from) {
                t.name.clone_from(to);
            }
            for t in &mut snap.tables {
                for fk in &mut t.foreign_keys {
                    if &fk.ref_table == from {
                        fk.ref_table.clone_from(to);
                    }
                }
            }
            snap.tables.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::AlterSchema { table, to, .. } => {
            if let Some(t) = table_mut(snap, table) {
                t.schema.clone_from(to);
            }
        }
        _ => {
            if let Some(table) = step.table_name().map(str::to_string) {
                if let Some(t) = table_mut(snap, &table) {
                    evolve_table(t, step);
                }
            }
        }
    }
}

fn table_mut<'a>(snap: &'a mut Snapshot, name: &str) -> Option<&'a mut TableSnap> {
    snap.tables.iter_mut().find(|t| t.name == name)
}

/// A step confined to one existing table.
fn evolve_table(t: &mut TableSnap, step: &Step) {
    match step {
        Step::AddColumn { column, .. } => {
            t.columns.retain(|c| c.name != column.name);
            t.columns.push(column.clone());
            t.columns.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::DropColumn { column, .. } => t.columns.retain(|c| &c.name != column),
        Step::AlterColumn { column, after, .. } => {
            if let Some(c) = t.columns.iter_mut().find(|c| &c.name == column) {
                *c = after.clone();
            }
        }
        Step::RenameColumn { from, to, .. } => {
            let rename = |c: &mut String| {
                if c == from {
                    c.clone_from(to);
                }
            };
            for c in &mut t.columns {
                rename(&mut c.name);
            }
            t.pk.iter_mut().for_each(rename);
            for i in &mut t.indexes {
                i.columns.iter_mut().for_each(rename);
            }
            for fk in &mut t.foreign_keys {
                fk.columns.iter_mut().for_each(rename);
            }
            if let Some((c, _)) = &mut t.soft_delete {
                rename(c);
            }
            t.columns.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::AddIndex { index, .. } | Step::AddUnique { index, .. } => {
            t.indexes.retain(|i| i.name != index.name);
            t.indexes.push(index.clone());
            t.indexes.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::DropIndex { name, .. } | Step::DropUnique { name, .. } => {
            t.indexes.retain(|i| &i.name != name);
        }
        Step::AddForeignKey { fk, .. } => {
            t.foreign_keys.retain(|f| f.columns != fk.columns);
            t.foreign_keys.push(fk.clone());
            t.foreign_keys.sort();
        }
        Step::DropForeignKey { columns, .. } => t.foreign_keys.retain(|f| &f.columns != columns),
        _ => {}
    }
}
