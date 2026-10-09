//! A closed registry owns model/file collisions and per-table column-name collisions.
use based_catalog::{Catalog, TableId, ValueGeneration};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ModelNames {
    pub physical: TableId,
    pub model: String,
    pub file: String,
    pub fields: BTreeMap<String, String>,
}

pub(crate) fn registry(catalog: &Catalog) -> BTreeMap<TableId, ModelNames> {
    let mut used_models = BTreeSet::new();
    catalog
        .tables
        .iter()
        .map(|table| {
            let base = super::identifier::model(&table.id.name);
            let model = allocate(&base, "", &mut used_models, true);
            let generated = table
                .primary_key
                .as_ref()
                .filter(|key| key.columns.len() == 1)
                .and_then(|key| {
                    table
                        .columns
                        .iter()
                        .find(|column| column.name == key.columns[0])
                })
                .filter(|column| column.generation != ValueGeneration::None)
                .map(|column| &column.name);
            let mut used_fields = BTreeSet::from(["id".into()]);
            let fields = table
                .columns
                .iter()
                .map(|column| {
                    let name = match generated == Some(&column.name) {
                        true => "id".into(),
                        false => allocate(
                            &super::identifier::field(&column.name),
                            "_",
                            &mut used_fields,
                            false,
                        ),
                    };
                    (column.name.clone(), name)
                })
                .collect();
            let names = ModelNames {
                physical: table.id.clone(),
                file: format!("{}.bsl", model.to_ascii_lowercase()),
                model,
                fields,
            };
            (table.id.clone(), names)
        })
        .collect()
}

fn allocate(base: &str, separator: &str, used: &mut BTreeSet<String>, folded: bool) -> String {
    let mut candidate = base.to_owned();
    let mut suffix = 2;
    loop {
        let key = match folded {
            true => candidate.to_ascii_lowercase(),
            false => candidate.clone(),
        };
        if used.insert(key) {
            return candidate;
        }
        candidate = format!("{base}{separator}{suffix}");
        suffix += 1;
    }
}
