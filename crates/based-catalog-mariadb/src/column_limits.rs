use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};
use sqlx::{mysql::MySqlRow, Row};

pub(crate) fn report(
    row: &MySqlRow,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<(), sqlx::Error> {
    let extra: String = row.try_get("EXTRA")?;
    if matches!(
        extra.as_str(),
        "" | "auto_increment" | "VIRTUAL GENERATED" | "STORED GENERATED" | "PERSISTENT GENERATED"
    ) {
        return Ok(());
    }
    findings.push(CatalogDiagnostic {
        table: id.clone(), member: Some(row.try_get("COLUMN_NAME")?), severity: CatalogSeverity::Error,
        code: CatalogCode::UnsupportedAttribute,
        message: "Additional native column attributes are retained in the table definition and require manual review".into(),
    });
    Ok(())
}
