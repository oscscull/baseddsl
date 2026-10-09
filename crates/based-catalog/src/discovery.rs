//! Validate and report a discovery result; findings never disappear behind a success flag.
use super::{Catalog, CatalogDiagnostic, CatalogSeverity, Selection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub catalog: Catalog,
    pub diagnostics: Vec<CatalogDiagnostic>,
}

impl Discovery {
    pub fn checked(
        catalog: Catalog,
        selection: &Selection,
        mut adapter_findings: Vec<CatalogDiagnostic>,
    ) -> Self {
        let catalog = catalog.canonicalize();
        adapter_findings.extend(super::validation::validate(&catalog, selection));
        adapter_findings.extend(super::capability::assess(&catalog));
        adapter_findings.sort();
        adapter_findings.dedup();
        Self {
            catalog,
            diagnostics: adapter_findings,
        }
    }

    /// A catalog with blocking findings is inspectable, not a complete supported import.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|finding| finding.severity == CatalogSeverity::Error)
    }
}
