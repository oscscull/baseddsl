use based_catalog::{
    CatalogCode, CatalogDiagnostic, CatalogSeverity, Table, TableKind, TypeFamily,
};

pub(crate) fn report(table: &Table, options: &str, diagnostics: &mut Vec<CatalogDiagnostic>) {
    let mut finding = |member: Option<String>, code, message: &str| {
        diagnostics.push(CatalogDiagnostic {
            severity: CatalogSeverity::Error,
            code,
            table: table.id.clone(),
            member,
            message: message.into(),
        });
    };
    if table.native_definition.is_none() {
        finding(None, CatalogCode::IncompleteMetadata, "Native object definition is hidden by metadata-only privileges; automatic import is blocked");
    }
    if table.kind == TableKind::Table && table.engine.as_deref() != Some("InnoDB") {
        finding(
            None,
            CatalogCode::UnsupportedAttribute,
            "Only MariaDB InnoDB base tables are evaluated",
        );
    }
    if !options.is_empty() {
        finding(
            None,
            CatalogCode::UnsupportedAttribute,
            "Native table options are retained in the definition and require manual review",
        );
    }
    for column in &table.columns {
        if column.native_type.family == TypeFamily::Enum {
            finding(Some(column.name.clone()), CatalogCode::IncompleteMetadata, "Native enum labels are retained in the declaration; automatic enum normalization is unsupported");
        }
        let declaration = &column.native_type.declaration;
        if declaration
            .split_ascii_whitespace()
            .any(|word| word == "zerofill")
        {
            finding(
                Some(column.name.clone()),
                CatalogCode::UnsupportedAttribute,
                "Native ZEROFILL semantics require manual representation",
            );
        }
    }
    for index in &table.indexes {
        if index.method.as_deref() != Some("BTREE") {
            finding(
                Some(index.name.clone()),
                CatalogCode::UnsupportedAttribute,
                "Native index method requires manual representation",
            );
        }
    }
}
