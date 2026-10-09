//! Verify compiler index field expansion against physical ordered key/index parts.
use super::ImportReport;
use based_catalog::{CatalogCode, IndexOrigin, IndexTarget, Table};
use based_sema::{CheckedSchema, RModel};

pub(crate) fn compare(
    table: &Table,
    schema: &CheckedSchema,
    model: &RModel,
    report: &mut ImportReport,
) {
    let mut expected: Vec<_> = table
        .unique_keys
        .iter()
        .map(|key| (key.columns.clone(), true))
        .collect();
    expected.extend(
        table
            .indexes
            .iter()
            .filter(|index| index.origin == IndexOrigin::Explicit)
            .map(|index| {
                (
                    index
                        .parts
                        .iter()
                        .filter_map(|part| match &part.target {
                            IndexTarget::Column(name) => Some(name.clone()),
                            IndexTarget::Expression(_) => None,
                        })
                        .collect::<Vec<_>>(),
                    index.unique,
                )
            }),
    );
    let mut actual: Vec<_> = model
        .indexes
        .iter()
        .map(|index| {
            let columns = index
                .columns
                .iter()
                .flat_map(|name| {
                    model.member(name).map_or_else(Vec::new, |member| {
                        let columns = schema.fk_columns(member);
                        if columns.is_empty() {
                            return vec![member.physical_col().to_owned()];
                        }
                        columns.into_iter().map(|(column, _)| column).collect()
                    })
                })
                .collect::<Vec<_>>();
            (columns, index.unique)
        })
        .collect();
    expected.sort();
    actual.sort();
    if expected != actual {
        report.error(
            &table.id,
            None,
            CatalogCode::InvalidReference,
            "Compiler-resolved ordered uniqueness/index mapping differs from the catalog",
        );
    }
}
