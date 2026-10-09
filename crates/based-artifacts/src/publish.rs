//! Preflight the complete set, stage all changed outputs, then publish each atomically.
use crate::{stage::stage, Artifact, Error, Format, Policy, Report};
use std::{collections::HashSet, fs, io::ErrorKind};

pub fn publish(artifacts: &[Artifact], policy: Policy) -> Result<Report, Error> {
    let mut report = Report::default();
    let mut changed = Vec::new();
    let mut paths = HashSet::new();
    for artifact in artifacts {
        if !paths.insert(&artifact.path) {
            return Err(Error::Invalid(format!(
                "duplicate artifact destination: {}",
                artifact.path.display()
            )));
        }
        if !artifact.format.owns(&artifact.bytes) {
            return Err(Error::Invalid(format!(
                "generated output has no ownership marker: {}",
                artifact.path.display()
            )));
        }
        match current(artifact, policy)? {
            true => report.unchanged.push(artifact.path.clone()),
            false => changed.push(artifact),
        }
    }
    if matches!(policy, Policy::Check) {
        if changed.is_empty() {
            return Ok(report);
        }
        return Err(Error::Stale(
            changed
                .iter()
                .map(|artifact| artifact.path.clone())
                .collect(),
        ));
    }
    // If any staging step fails, dropping the staged set removes every temporary file.
    let staged = changed
        .into_iter()
        .map(stage)
        .collect::<Result<Vec<_>, _>>()?;
    for output in staged {
        // Recheck ownership after staging in case another writer changed the target.
        match current(output.artifact, policy) {
            Ok(true) => {
                report.unchanged.push(output.artifact.path.clone());
                continue;
            }
            Ok(false) => {}
            Err(source) => {
                return Err(Error::Partial {
                    written: report.written,
                    source: Box::new(source),
                })
            }
        }
        if let Err(error) = output.file.persist(&output.artifact.path) {
            return Err(Error::Partial {
                written: report.written,
                source: Box::new(Error::io("replacing", &output.artifact.path, error.error)),
            });
        }
        report.written.push(output.artifact.path.clone());
    }
    Ok(report)
}

fn current(artifact: &Artifact, policy: Policy) -> Result<bool, Error> {
    let metadata = match fs::symlink_metadata(&artifact.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::io("reading", &artifact.path, error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::Invalid(format!(
            "artifact destination must be a regular file: {}",
            artifact.path.display()
        )));
    }
    let bytes =
        fs::read(&artifact.path).map_err(|error| Error::io("reading", &artifact.path, error))?;
    if bytes == artifact.bytes {
        return Ok(true);
    }
    if matches!(
        policy,
        Policy::Write {
            overwrite_user_owned: false
        }
    ) && !Format::owns_any(&bytes)
    {
        return Err(Error::UserOwned(artifact.path.clone()));
    }
    Ok(false)
}
