//! Distinguish true INTEGER PRIMARY KEY aliases from indexed/DESC/WITHOUT ROWID keys.
use based_catalog::{Column, Index, IndexOrigin, Key, ValueGeneration};

pub(crate) fn apply(
    columns: &mut [Column],
    primary: &Option<Key>,
    indexes: &[Index],
    without_rowid: bool,
    autoincrement: bool,
) {
    if without_rowid
        || indexes
            .iter()
            .any(|index| index.origin == IndexOrigin::PrimaryKey)
    {
        return;
    }
    let Some(key) = primary.as_ref().filter(|key| key.columns.len() == 1) else {
        return;
    };
    let Some(column) = columns
        .iter_mut()
        .find(|column| column.name == key.columns[0])
    else {
        return;
    };
    if !column
        .native_type
        .declaration
        .eq_ignore_ascii_case("INTEGER")
    {
        return;
    }
    column.nullable = false;
    column.generation = ValueGeneration::SqliteRowId { autoincrement };
}
