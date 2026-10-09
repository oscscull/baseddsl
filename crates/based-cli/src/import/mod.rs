//! Coordinate a single metadata read, complete source check and exclusive publication.
mod check;
mod connection;
pub mod options;
mod paths;
mod publish;
mod report;
mod selection;
use crate::{error::CliError, project, project_root};

pub async fn execute(options: options::Options) -> Result<(), CliError> {
    let root = project_root::resolve(options.root.as_deref())?;
    let project = project::discover_allow_empty_project(&root)?;
    let selected = selection::parse(&options.tables)?;
    let output = paths::output(&root, &project.manifest, options.output.as_deref())?;
    let discovery = connection::discover(
        &root,
        based_codegen::Dialect::parse(&project.manifest.dialect),
        options.database_url,
        &selected,
    )
    .await?;
    let mut emission = match based_catalog_bsl::emit(&discovery, &selected, &project.manifest) {
        Ok(emission) => emission,
        Err(report) => {
            report::write("blocked", &discovery.catalog, &report, &[], options.json)?;
            return Err(CliError::summary(
                true,
                "import blocked by catalog/model findings; no models written",
            ));
        }
    };
    if let Err(error) = publish::preflight(&output, &emission.files) {
        report::write(
            "blocked",
            &discovery.catalog,
            &emission.report,
            &[],
            options.json,
        )?;
        return Err(error);
    }
    emission.report.compiler = match check::combined(project, &output, &emission.files) {
        Ok(report) => Some(report),
        Err(check::Error::Compiler(report)) => {
            emission.report.compiler = Some(report);
            report::write(
                "blocked",
                &discovery.catalog,
                &emission.report,
                &[],
                options.json,
            )?;
            return Err(CliError::summary(
                true,
                "imported and existing models do not check together; no models written",
            ));
        }
        Err(check::Error::Io(error)) => return Err(error),
    };
    match publish::write(&output, &emission.files) {
        Ok(written) => report::write(
            "imported",
            &discovery.catalog,
            &emission.report,
            &written,
            options.json,
        ),
        Err(failure) => {
            let status = if failure.written.is_empty() {
                "blocked"
            } else {
                "partial"
            };
            report::write(
                status,
                &discovery.catalog,
                &emission.report,
                &failure.written,
                options.json,
            )?;
            Err(failure.error)
        }
    }
}
