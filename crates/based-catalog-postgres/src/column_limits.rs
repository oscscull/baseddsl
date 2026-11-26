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
            .is_some_and(|options| options.iter().any(|option| !server_stamp(option)))
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

/// PostgreSQL 16.16+ records `pg_attver` on every column; it is not a user-set option.
fn server_stamp(option: &str) -> bool {
    option.starts_with("pg_attver=")
}
