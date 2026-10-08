use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};
use sqlx::{postgres::PgRow, Row};

pub(crate) fn report(
    row: &PgRow,
    id: &TableId,
    name: &str,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<(), sqlx::Error> {
    for attribute in ["attoptions", "attfdwoptions"] {
        if row
            .try_get::<Option<Vec<String>>, _>(attribute)?
            .is_some_and(|options| !options.is_empty())
        {
            findings.push(CatalogDiagnostic {
                table: id.clone(),
                member: Some(name.into()),
                severity: CatalogSeverity::Error,
                code: CatalogCode::UnsupportedAttribute,
                message: "Native column options require manual representation".into(),
            });
        }
    }
    Ok(())
}
