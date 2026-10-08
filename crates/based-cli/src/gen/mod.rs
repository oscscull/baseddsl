//! Generation command orchestration; rendering and publication have separate owners.
mod command;
pub mod options;
mod output;
mod path;
mod prepare;
mod render;
mod sql;

use crate::{error::CliError, project::load_checked, project_root};
use based_manifest::ClientMode;
use options::{GenTarget, Mode, OutputOptions};
use render::Kind;
use std::path::Path;

pub fn execute(target: GenTarget) -> Result<(), CliError> {
    match target {
        GenTarget::Sql { root, output } => one(
            Kind::Sql,
            &project_root::resolve(root.as_deref())?,
            output,
            None,
        ),
        GenTarget::Openapi { root, output } => one(
            Kind::OpenApi,
            &project_root::resolve(root.as_deref())?,
            output,
            None,
        ),
        GenTarget::Client {
            root,
            output,
            embedded,
            mode,
        } => {
            let mode = mode
                .map(|mode| match mode {
                    Mode::Wire => ClientMode::Wire,
                    Mode::Embedded => ClientMode::Embedded,
                })
                .or_else(|| embedded.then_some(ClientMode::Embedded));
            one(
                Kind::Client,
                &project_root::resolve(root.as_deref())?,
                output,
                mode,
            )
        }
        GenTarget::All { root, check, force } => all(
            &project_root::resolve(root.as_deref())?,
            OutputOptions {
                out: None,
                check,
                force,
            },
        ),
    }
}

fn one(
    kind: Kind,
    root: &Path,
    options: OutputOptions,
    mode: Option<ClientMode>,
) -> Result<(), CliError> {
    let loaded = load_checked(root)?;
    let mode = mode.unwrap_or(loaded.0.manifest.generate.client_mode);
    let destination = options
        .out
        .as_deref()
        .or_else(|| kind.configured(&loaded).map(Path::new));
    let Some(destination) = destination else {
        return output::stdout(&prepare::bytes(kind, &loaded, mode, None)?, &options);
    };
    let artifact = prepare::file(kind, root, destination, &loaded, mode)?;
    output::publish(&[artifact], options.policy())
}

fn all(root: &Path, options: OutputOptions) -> Result<(), CliError> {
    let loaded = load_checked(root)?;
    let mode = loaded.0.manifest.generate.client_mode;
    let artifacts = [Kind::Client, Kind::Sql, Kind::OpenApi]
        .into_iter()
        .filter_map(|kind| kind.configured(&loaded).map(|out| (kind, Path::new(out))))
        .map(|(kind, destination)| prepare::file(kind, root, destination, &loaded, mode))
        .collect::<Result<Vec<_>, CliError>>()?;
    if artifacts.is_empty() {
        return Err(CliError::usage(
            "no [generate] destinations configured in based.toml",
        ));
    }
    output::publish(&artifacts, options.policy())
}
