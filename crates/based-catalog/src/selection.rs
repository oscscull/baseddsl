//! An explicit closed set of physical tables; discovering dependencies never expands it.
use super::TableId;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    tables: BTreeSet<TableId>,
}

impl Selection {
    pub fn new(tables: impl IntoIterator<Item = TableId>) -> Result<Self, &'static str> {
        let tables: BTreeSet<_> = tables.into_iter().collect();
        if tables.is_empty() {
            return Err("Select at least one physical table");
        }
        if tables.iter().any(|id| {
            id.namespace.is_empty()
                || id.name.is_empty()
                || id.namespace.contains('\0')
                || id.name.contains('\0')
        }) {
            return Err("Table identities need nonempty namespace/name without NUL");
        }
        Ok(Self { tables })
    }

    pub fn contains(&self, id: &TableId) -> bool {
        self.tables.contains(id)
    }
    pub fn tables(&self) -> impl Iterator<Item = &TableId> {
        self.tables.iter()
    }
}
