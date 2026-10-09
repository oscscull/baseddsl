use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};
use sqlx::{postgres::PgRow, Row};

pub(crate) fn report(
    row: &PgRow,
    id: &TableId,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<(), sqlx::Error> {
    let name: String = row.try_get("name")?;
    let mut finding = |severity, message: &str| {
        findings.push(CatalogDiagnostic {
            table: id.clone(),
            member: Some(name.clone()),
            severity,
            code: CatalogCode::UnsupportedAttribute,
            message: message.into(),
        });
    };
    for (attribute, message) in [
        (
            "indnullsnotdistinct",
            "Native NULLS NOT DISTINCT uniqueness requires manual representation",
        ),
        (
            "indisexclusion",
            "Native exclusion constraint requires manual representation",
        ),
        (
            "custom_tablespace",
            "Native index tablespace requires manual representation",
        ),
    ] {
        if row.try_get::<bool, _>(attribute)? {
            finding(CatalogSeverity::Error, message);
        }
    }
    if row.try_get::<String, _>("method")? != "btree" {
        finding(
            CatalogSeverity::Error,
            "Non-btree index method requires manual representation",
        );
    }
    if row.try_get::<Option<bool>, _>("opcdefault")? == Some(false) {
        finding(
            CatalogSeverity::Error,
            "Non-default index operator class requires manual representation",
        );
    }
    if row
        .try_get::<Option<Vec<String>>, _>("reloptions")?
        .is_some_and(|options| !options.is_empty())
    {
        finding(
            CatalogSeverity::Error,
            "Native index storage options require manual representation",
        );
    }
    let descending = row
        .try_get::<Option<bool>, _>("descending")?
        .unwrap_or(false);
    if row
        .try_get::<Option<bool>, _>("nulls_first")?
        .is_some_and(|first| first != descending)
    {
        finding(
            CatalogSeverity::Error,
            "Non-default index NULLS ordering requires manual representation",
        );
    }
    for (attribute, message) in [
        (
            "indisclustered",
            "Index has native clustering state, which import does not reproduce",
        ),
        (
            "indisreplident",
            "Index is native replica identity, which import does not reproduce",
        ),
    ] {
        if row.try_get::<bool, _>(attribute)? {
            finding(CatalogSeverity::Warning, message);
        }
    }
    Ok(())
}
