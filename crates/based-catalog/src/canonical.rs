//! Canonicalize unordered catalog collections while retaining semantic column/key order.
use super::{Catalog, Table};

impl Catalog {
    pub fn canonicalize(mut self) -> Self {
        self.tables.sort_by(|a, b| a.id.cmp(&b.id));
        self.tables.iter_mut().for_each(Table::canonicalize);
        self
    }
}

impl Table {
    fn canonicalize(&mut self) {
        self.columns.sort_by_key(|column| column.position);
        self.unique_keys
            .sort_by(|a, b| (&a.name, &a.columns).cmp(&(&b.name, &b.columns)));
        self.indexes.sort_by(|a, b| a.name.cmp(&b.name));
        self.foreign_keys.sort_by(|a, b| {
            (&a.name, &a.target, &a.columns, &a.target_columns).cmp(&(
                &b.name,
                &b.target,
                &b.columns,
                &b.target_columns,
            ))
        });
        self.checks
            .sort_by(|a, b| (&a.name, &a.expression).cmp(&(&b.name, &b.expression)));
    }
}
