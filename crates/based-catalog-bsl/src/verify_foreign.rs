//! Compare resolved FK pairs, targets and actions to declared catalog constraints.
use super::ImportReport;
use based_catalog::{CatalogCode, Table};
use based_sema::{CheckedSchema, ForeignKeys, MemberKind, RModel};

pub(crate) fn compare(
    table: &Table,
    schema: &CheckedSchema,
    model: &RModel,
    foreign_keys: ForeignKeys,
    report: &mut ImportReport,
) {
    let actual_foreign: Vec<_> = model
        .members
        .iter()
        .filter_map(|member| {
            let MemberKind::Forward { target, .. } = &member.kind else {
                return None;
            };
            let target = schema.model(target)?;
            let foreign = model.resolved_fk(member, foreign_keys)?;
            let pairs = schema.fk_columns(member);
            Some((
                pairs
                    .iter()
                    .map(|(column, _)| column.clone())
                    .collect::<Vec<_>>(),
                target.table.as_str(),
                target.schema.as_deref().unwrap_or("main"),
                pairs
                    .iter()
                    .map(|(_, member)| member.physical_col().to_owned())
                    .collect::<Vec<_>>(),
                foreign
                    .on_delete
                    .map_or("no_action", based_sema::FkAction::snap),
                foreign
                    .on_update
                    .map_or("no_action", based_sema::FkAction::snap),
            ))
        })
        .collect();
    let expected_foreign: Vec<_> = table
        .foreign_keys
        .iter()
        .map(|foreign| {
            (
                foreign.columns.clone(),
                foreign.target.name.as_str(),
                foreign.target.namespace.as_str(),
                foreign.target_columns.clone(),
                super::relations::action(foreign.on_delete),
                super::relations::action(foreign.on_update),
            )
        })
        .collect();
    let mut actual_foreign = actual_foreign;
    let mut expected_foreign = expected_foreign;
    actual_foreign.sort();
    expected_foreign.sort();
    if actual_foreign != expected_foreign {
        report.error(
            &table.id,
            None,
            CatalogCode::InvalidReference,
            "Compiler-resolved foreign-key pairs/target/actions differ from the catalog",
        );
    }
}
