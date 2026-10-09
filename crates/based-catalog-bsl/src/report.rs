//! One complete import report; compiler findings remain distinct from physical losses.
use based_catalog::{CatalogCode, CatalogDiagnostic, CatalogSeverity, TableId};

#[derive(Debug, Default)]
pub struct ImportReport {
    pub diagnostics: Vec<CatalogDiagnostic>,
    pub compiler: Option<based_project::Report>,
}

impl ImportReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|finding| finding.severity == CatalogSeverity::Error)
            || self
                .compiler
                .as_ref()
                .is_some_and(|report| report.errors() != 0)
    }

    pub(crate) fn error(
        &mut self,
        table: &TableId,
        member: Option<&str>,
        code: CatalogCode,
        message: impl Into<String>,
    ) {
        self.add(table, member, CatalogSeverity::Error, code, message.into());
    }

    pub(crate) fn loss(
        &mut self,
        table: &TableId,
        member: Option<&str>,
        message: impl Into<String>,
    ) {
        self.add(
            table,
            member,
            CatalogSeverity::Warning,
            CatalogCode::DefinitionLoss,
            message.into(),
        );
    }

    fn add(
        &mut self,
        table: &TableId,
        member: Option<&str>,
        severity: CatalogSeverity,
        code: CatalogCode,
        message: String,
    ) {
        self.diagnostics.push(CatalogDiagnostic {
            table: table.clone(),
            member: member.map(str::to_owned),
            severity,
            code,
            message,
        });
    }

    pub(crate) fn canonicalize(&mut self) {
        self.diagnostics.sort();
        self.diagnostics.dedup();
    }
}
