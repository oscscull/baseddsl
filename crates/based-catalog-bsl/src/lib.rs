//! Pure catalog-to-BSL emission; no connections, application data or file publication.
mod compiler;
mod defaults;
mod generation;
mod identifier;
mod indexes;
mod model;
mod names;
mod preflight;
mod relations;
mod report;
mod types;
mod verify;
mod verify_foreign;
mod verify_indexes;

use based_catalog::{Discovery, Selection};
use based_manifest::Manifest;
use based_project::{CheckedProject, Sources};
pub use names::ModelNames;
pub use report::ImportReport;
use std::{collections::BTreeMap, path::Path};

pub struct Emission {
    /// Relative to the configured schema root. The caller exclusively publishes these files.
    pub files: Sources,
    pub names: BTreeMap<based_catalog::TableId, ModelNames>,
    pub checked: CheckedProject,
    pub report: ImportReport,
}

/// All selected models are checked together; an error returns a report and no publishable files.
pub fn emit(
    discovery: &Discovery,
    selection: &Selection,
    manifest: &Manifest,
) -> Result<Emission, ImportReport> {
    let discovery = Discovery::checked(
        discovery.catalog.clone(),
        selection,
        discovery.diagnostics.clone(),
    );
    let catalog = &discovery.catalog;
    let mut report = ImportReport {
        diagnostics: discovery.diagnostics,
        compiler: None,
    };
    preflight::check(catalog, manifest, &mut report);
    report.canonicalize();
    if report.has_errors() {
        return Err(report);
    }
    let names = names::registry(catalog);
    let mut files = Vec::new();
    for table in &catalog.tables {
        let source = model::render(catalog, table, &names, manifest, &mut report);
        files.push((Path::new(&names[&table.id].file).to_owned(), source));
    }
    report.canonicalize();
    if report.has_errors() {
        return Err(report);
    }
    let checked = match compiler::check(&mut files, manifest) {
        Ok(checked) => checked,
        Err(compiler) => {
            report.compiler = Some(compiler);
            return Err(report);
        }
    };
    verify::physical(
        catalog,
        &checked.schema,
        &names,
        based_sema::ForeignKeys::parse(&manifest.schema.foreign_keys),
        &mut report,
    );
    report.canonicalize();
    if report.has_errors() {
        return Err(report);
    }
    report.compiler = Some(checked.report.clone());
    Ok(Emission {
        files,
        names,
        checked,
        report,
    })
}
