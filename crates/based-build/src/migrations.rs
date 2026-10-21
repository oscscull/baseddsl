//! Fail the build on the same migration problems as `based migrate verify`.
use crate::Error;
use based_project::{CheckedProject, MigrationFiles, Severity};

pub(super) fn verify(checked: &CheckedProject, migrations: &[MigrationFiles]) -> Result<(), Error> {
    let sources: Vec<_> = migrations.iter().map(MigrationFiles::source).collect();
    let findings =
        based_project::verify_migrations(&checked.schema, &checked.project.manifest, &sources);
    let mut problems = Vec::new();
    for finding in findings {
        match finding.severity {
            Severity::Problem => problems.push(finding),
            Severity::Warning => println!("cargo:warning=migrations/{finding}"),
            Severity::Note => {}
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Migrations(problems))
    }
}
