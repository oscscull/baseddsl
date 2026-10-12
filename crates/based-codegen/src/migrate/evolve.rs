//! Advance a [`Snapshot`] by one structural [`Step`] — the schema state at a given line of an
//! `up.mig`, which data steps are checked against and SQLite rebuilds target.

use super::diff::Step;
use super::model::{Snapshot, TableSnap};

/// Apply `step` to `snap` in place. Steps that emit no schema change (scope notes, `raw`)
/// leave it untouched.
pub(crate) fn evolve(snap: &mut Snapshot, step: &Step) {
    match step {
        Step::CreateTable(t) => put_named(&mut snap.tables, t, |t| &t.name),
        Step::DropTable { name, .. } => snap.tables.retain(|t| &t.name != name),
        Step::RenameTable { from, to, .. } => {
            if let Some(t) = table_mut(snap, from) {
                t.name.clone_from(to);
            }
            snap.tables
                .iter_mut()
                .flat_map(|t| &mut t.foreign_keys)
                .filter(|fk| &fk.ref_table == from)
                .for_each(|fk| fk.ref_table.clone_from(to));
            snap.tables.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::AlterSchema { table, to, .. } => {
            if let Some(t) = table_mut(snap, table) {
                t.schema.clone_from(to);
            }
        }
        _ => {
            if let Some(t) = step.table_name().and_then(|n| table_mut(snap, n)) {
                evolve_table(t, step);
            }
        }
    }
}

fn table_mut<'a>(snap: &'a mut Snapshot, name: &str) -> Option<&'a mut TableSnap> {
    snap.tables.iter_mut().find(|t| t.name == name)
}

/// Replace the entry named like `item` (or add it), keeping `v` sorted by name.
fn put_named<T: Clone>(v: &mut Vec<T>, item: &T, name: impl for<'x> Fn(&'x T) -> &'x String) {
    v.retain(|x| name(x) != name(item));
    v.push(item.clone());
    v.sort_by(|a, b| name(a).cmp(name(b)));
}

/// A step confined to one existing table.
fn evolve_table(t: &mut TableSnap, step: &Step) {
    match step {
        Step::AddColumn { column, .. } => put_named(&mut t.columns, column, |c| &c.name),
        Step::DropColumn { column, .. } => t.columns.retain(|c| &c.name != column),
        Step::AlterColumn { column, after, .. } => {
            if let Some(c) = t.columns.iter_mut().find(|c| &c.name == column) {
                *c = after.clone();
            }
        }
        Step::RenameColumn { from, to, .. } => {
            t.columns
                .iter_mut()
                .map(|c| &mut c.name)
                .chain(&mut t.pk)
                .chain(t.indexes.iter_mut().flat_map(|i| &mut i.columns))
                .chain(t.foreign_keys.iter_mut().flat_map(|fk| &mut fk.columns))
                .chain(t.soft_delete.iter_mut().map(|(c, _)| c))
                .filter(|c| *c == from)
                .for_each(|c| c.clone_from(to));
            t.columns.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Step::AddIndex { index, .. } | Step::AddUnique { index, .. } => {
            put_named(&mut t.indexes, index, |i| &i.name);
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
