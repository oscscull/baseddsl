use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};
use sqlx::{postgres::PgRow, Row};

pub(crate) fn report(
    row: &PgRow,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<(), sqlx::Error> {
    let mut finding = |message: &str| {
        findings.push(CatalogDiagnostic {
            table: id.clone(),
            member: None,
            severity: CatalogSeverity::Error,
            code: CatalogCode::UnsupportedAttribute,
            message: message.into(),
        });
    };
    for (attribute, message) in [
        (
            "relrowsecurity",
            "Native row-security policy requires manual representation",
        ),
        (
            "relforcerowsecurity",
            "Forced native row-security policy requires manual representation",
        ),
        (
            "relispartition",
            "Native partition requires manual representation",
        ),
        (
            "inherits",
            "Native partitioning/inheritance requires manual representation",
        ),
    ] {
        if row.try_get::<bool, _>(attribute)? {
            finding(message);
        }
    }
    if row.try_get::<String, _>("kind")? == "p" {
        finding("Native partitioned table requires manual representation");
    }
    if row.try_get::<String, _>("persistence")? != "p" {
        finding("Native temporary/unlogged table requires manual representation");
    }
    if row
        .try_get::<Option<Vec<String>>, _>("reloptions")?
        .is_some_and(|options| !options.is_empty())
    {
        finding("Native table storage options require manual representation");
    }
    Ok(())
}
