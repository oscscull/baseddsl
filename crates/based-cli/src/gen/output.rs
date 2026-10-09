//! Map artifact publication outcomes onto concise CLI output and exit classes.
use super::options::OutputOptions;
use crate::error::CliError;
use based_artifacts::{Artifact, Error, Policy};
use std::io::Write;

pub(super) fn error(error: Error) -> CliError {
    match error {
        Error::UserOwned(_) | Error::Invalid(_) => CliError::usage(error.to_string()),
        _ => CliError::caused_by("generated artifact publication failed", error),
    }
}

pub(super) fn stdout(bytes: &[u8], options: &OutputOptions) -> Result<(), CliError> {
    if options.check || options.force {
        return Err(CliError::usage(
            "--check/--force requires an output path (-o or [generate] configuration)",
        ));
    }
    std::io::stdout()
        .write_all(bytes)
        .map_err(|source| CliError::io("writing stdout", source))
}

pub(super) fn publish(artifacts: &[Artifact], policy: Policy) -> Result<(), CliError> {
    let report = based_artifacts::publish(artifacts, policy).map_err(error)?;
    for path in report.written {
        eprintln!("wrote {}", path.display());
    }
    for path in report.unchanged {
        eprintln!("current {}", path.display());
    }
    eprintln!("Check freshness with the same generation command and --check.");
    Ok(())
}
