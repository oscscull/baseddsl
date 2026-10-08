use super::keywords::Keywords;
use based_catalog::{
    CatalogCode, CatalogDiagnostic, CatalogSeverity, Deferral, IndexTarget, Table, ValueGeneration,
};

pub(crate) fn report(table: &Table, keywords: &Keywords, findings: &mut Vec<CatalogDiagnostic>) {
    let mut finding = |member: Option<String>, code, message: &str| {
        findings.push(CatalogDiagnostic {
            table: table.id.clone(),
            member,
            severity: CatalogSeverity::Error,
            code,
            message: message.into(),
        });
    };
    for (keyword, message) in [
        ("COLLATE", "Column collation declarations are retained in native DDL; individual normalization is unavailable"),
        ("CHECK", "CHECK expressions are retained in native DDL; individual normalization is unavailable"),
        ("MATCH", "Declared MATCH syntax is retained in native DDL; SQLite enforces SIMPLE matching"),
        ("CONFLICT", "Native ON CONFLICT policy is retained in DDL and requires manual representation"),
    ] {
        if keywords.has(keyword) { finding(None, CatalogCode::IncompleteMetadata, message); }
    }
    for column in &table.columns {
        if matches!(column.generation, ValueGeneration::Generated { .. }) {
            finding(Some(column.name.clone()), CatalogCode::IncompleteMetadata, "Generated expression is retained in native DDL; individual expression normalization is unavailable");
        }
    }
    if table.primary_key.as_ref().is_some_and(|key| {
        key.columns.iter().any(|name| {
            table
                .columns
                .iter()
                .any(|column| &column.name == name && column.nullable)
        })
    }) {
        finding(
            None,
            CatalogCode::UnsupportedAttribute,
            "SQLite nullable primary-key semantics cannot be represented by a required BSL key",
        );
    }
    for foreign in &table.foreign_keys {
        if foreign.deferral == Deferral::Unknown {
            finding(None, CatalogCode::IncompleteMetadata, "Foreign-key timing declarations are retained in native DDL; PRAGMAs cannot expose per-key deferral");
        }
    }
    for index in &table.indexes {
        if index
            .parts
            .iter()
            .any(|part| matches!(part.target, IndexTarget::Expression(_)))
        {
            finding(Some(index.name.clone()), CatalogCode::IncompleteMetadata, "Index expressions are retained in native DDL; individual expression normalization is unavailable");
        }
    }
    if keywords.has("CONSTRAINT") {
        findings.push(CatalogDiagnostic {
            table: table.id.clone(),
            member: None,
            severity: CatalogSeverity::Warning,
            code: CatalogCode::DefinitionLoss,
            message:
                "Constraint names remain in native DDL; normalized SQLite keys/FKs may be unnamed"
                    .into(),
        });
    }
}
