use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};
use sqlx::{postgres::PgRow, Row};

/// Options the server stamps on every column itself (Postgres 16.16+), not set by the schema.
const SERVER_MANAGED: &[&str] = &["pg_attver"];

pub(crate) fn report(
    row: &PgRow,
    id: &TableId,
    name: &str,
    findings: &mut Vec<CatalogDiagnostic>,
) -> Result<(), sqlx::Error> {
    for attribute in ["attoptions", "attfdwoptions"] {
        if row
            .try_get::<Option<Vec<String>>, _>(attribute)?
            .is_some_and(|options| has_declared(&options))
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

fn has_declared(options: &[String]) -> bool {
    options.iter().any(|option| {
        let key = option
            .split_once('=')
            .map_or(option.as_str(), |(key, _)| key);
        !SERVER_MANAGED.contains(&key)
    })
}

#[cfg(test)]
mod tests {
    use super::has_declared;

    #[test]
    fn server_managed_options_are_not_declared() {
        assert!(!has_declared(&["pg_attver=26557696".into()]));
        assert!(has_declared(&["n_distinct=-1".into()]));
        assert!(has_declared(&[
            "pg_attver=26557696".into(),
            "n_distinct=-1".into()
        ]));
    }
}
