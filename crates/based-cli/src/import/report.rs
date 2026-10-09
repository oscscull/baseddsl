//! Render catalog losses and compiler findings without connection URLs or driver payloads.
use crate::error::CliError;
use based_catalog::{Catalog, CatalogDiagnostic, CatalogSeverity};
use based_catalog_bsl::ImportReport;
use serde::Serialize;
use std::{io::Write, path::PathBuf};

#[derive(Serialize)]
struct JsonReport<'a> {
    status: &'a str,
    catalog: &'a Catalog,
    diagnostics: &'a [CatalogDiagnostic],
    compiler: Vec<CompilerFinding<'a>>,
    written: &'a [PathBuf],
    migration_ownership: &'static str,
}

#[derive(Serialize)]
struct CompilerFinding<'a> {
    code: &'a str,
    severity: &'static str,
    message: &'a str,
    notes: &'a [String],
}

pub(super) fn write(
    status: &str,
    catalog: &Catalog,
    report: &ImportReport,
    written: &[PathBuf],
    json: bool,
) -> Result<(), CliError> {
    if json {
        return write_json(status, catalog, report, written);
    }
    eprintln!(
        "import {status}: {:?} {} ({} selected objects)",
        catalog.source.dialect,
        catalog.source.server_version,
        catalog.tables.len()
    );
    for finding in &report.diagnostics {
        let severity = match finding.severity {
            CatalogSeverity::Error => "error",
            CatalogSeverity::Warning => "warning",
        };
        let member = finding
            .member
            .as_deref()
            .map_or_else(String::new, |member| format!(".{member}"));
        eprintln!(
            "{severity}[{:?}] {}.{}{member}: {}",
            finding.code, finding.table.namespace, finding.table.name, finding.message
        );
    }
    if let Some(compiler) = &report.compiler {
        crate::render::render(&compiler.diagnostics, &compiler.sources);
    }
    for file in written {
        println!("imported {}", file.display());
    }
    if status == "imported" {
        eprintln!("Review native-definition warnings before future DDL. Existing migration ownership remains external; no baseline or SQL was applied.");
    }
    Ok(())
}

fn write_json(
    status: &str,
    catalog: &Catalog,
    report: &ImportReport,
    written: &[PathBuf],
) -> Result<(), CliError> {
    let compiler = report.compiler.as_ref().map_or_else(Vec::new, |report| {
        report
            .diagnostics
            .iter()
            .map(|diagnostic| CompilerFinding {
                code: diagnostic.code,
                severity: match diagnostic.severity {
                    based_diagnostics::Severity::Error => "error",
                    based_diagnostics::Severity::Warning => "warning",
                },
                message: &diagnostic.message,
                notes: &diagnostic.notes,
            })
            .collect()
    });
    let report = JsonReport {
        status,
        catalog,
        diagnostics: &report.diagnostics,
        compiler,
        written,
        migration_ownership: "external; no baseline or DDL execution",
    };
    let mut bytes = serde_json::to_vec_pretty(&report)
        .map_err(|error| CliError::io("serializing import report", error))?;
    bytes.push(b'\n');
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|error| CliError::io("writing import report", error))
}
