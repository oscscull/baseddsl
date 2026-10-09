use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};
use sqlx::{postgres::PgConnection, Row};

pub(crate) async fn report(
    conn: &mut PgConnection,
    oid: i64,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe("SELECT conname::text AS name, contype::text AS kind, convalidated, confdelsetcols IS NOT NULL AS partial_action FROM pg_catalog.pg_constraint WHERE conrelid = $1::bigint::oid ORDER BY conname"))
        .bind(oid).fetch_all(conn).await?;
    for row in rows {
        for (unsupported, message) in [
            (!row.try_get::<bool, _>("convalidated")?, "Unvalidated native constraint requires manual representation"),
            (row.try_get::<bool, _>("partial_action")?, "Foreign-key delete action applies to a subset of columns; manual representation is required"),
            (!matches!(row.try_get::<String, _>("kind")?.as_str(), "p" | "u" | "f" | "c"), "Native exclusion/constraint-trigger semantics require manual representation"),
        ] {
            if unsupported {
                findings.push(CatalogDiagnostic { table: id.clone(), member: Some(row.try_get("name")?),
                    severity: CatalogSeverity::Error, code: CatalogCode::UnsupportedAttribute, message: message.into() });
            }
        }
    }
    Ok(())
}
