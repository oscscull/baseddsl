//! Resolve `child.any(…)` in a data step: the child table and the columns tying its rows to
//! the parent, read off the snapshot's `fk` lines and `fk=<Model>` column markers.

use super::super::model::Snapshot;
use super::table::TableView;

/// The child table named by `child` and its `(child_col, parent_col)` link pairs to `parent`.
pub(crate) fn child_link<'a>(
    snap: &'a Snapshot,
    parent: &TableView,
    child: &str,
) -> Result<(TableView<'a>, Vec<(String, String)>), String> {
    let Some(view) = TableView::find(snap, child) else {
        return Err(format!("unknown table `{child}`"));
    };
    let mut links: Vec<Vec<(String, String)>> = view
        .snap
        .foreign_keys
        .iter()
        .filter(|fk| fk.ref_table == parent.name())
        .map(|fk| {
            fk.columns
                .iter()
                .cloned()
                .zip(fk.ref_columns.iter().cloned())
                .collect()
        })
        .collect();
    if let [pk] = parent.pk().as_slice() {
        for c in &view.snap.columns {
            let names_parent = c
                .fk
                .as_deref()
                .is_some_and(|m| m == parent.name() || based_sema::snake_case(m) == parent.name());
            let covered = links
                .iter()
                .any(|l| l.iter().any(|(col, _)| col == &c.name));
            if names_parent && !covered {
                links.push(vec![(c.name.clone(), pk.clone())]);
            }
        }
    }
    match links.len() {
        1 => Ok((view, links.remove(0))),
        0 => Err(format!(
            "table `{child}` has no foreign key to `{}`",
            parent.name()
        )),
        _ => Err(format!(
            "table `{child}` links to `{}` through more than one foreign key ({}) — `.any` needs exactly one",
            parent.name(),
            links
                .iter()
                .map(|l| l.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join("+"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}
