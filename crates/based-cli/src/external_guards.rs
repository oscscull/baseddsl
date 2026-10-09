//! Operator-owned callback configuration; separate from transport and listener lifecycle.
use crate::error::CliError;
use based_runtime::{external_guard::HttpGuard, Compiled, Guards};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    guards: BTreeMap<String, Mapping>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    endpoint: String,
    secret_env: String,
    #[serde(default = "default_deadline")]
    deadline_ms: u64,
}

fn default_deadline() -> u64 {
    2000
}

pub fn build(
    root: &Path,
    compiled: &Compiled,
    options: &crate::serve_options::GuardOptions,
) -> Result<Guards, CliError> {
    let guards = match &options.guard_config {
        Some(path) => load(&root.join(path))?.guards.into_iter().try_fold(
            Guards::new(),
            |guards, (name, mapping)| {
                mapping
                    .callback(name, compiled, options.guard_allow_loopback_http)
                    .map(|callback| callback.register(guards))
            },
        )?,
        None => Guards::new(),
    };
    let missing = guards.missing_for(compiled);
    if !missing.is_empty() {
        return Err(CliError::usage(format!(
            "{}; supply --guard-config <file>",
            based_runtime::GuardSetupError { missing }
        )));
    }
    Ok(guards)
}

fn load(path: &Path) -> Result<Config, CliError> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| CliError::io("reading guard configuration", e))?;
    toml::from_str(&source).map_err(|_| CliError::usage("invalid guard configuration; expected [guards.NAME] with endpoint, secret_env and optional deadline_ms (values redacted)"))
}

impl Mapping {
    fn callback(
        self,
        name: String,
        compiled: &Compiled,
        allow_loopback_http: bool,
    ) -> Result<HttpGuard, CliError> {
        if !compiled
            .declared_guards()
            .any(|(_, declared)| declared == name)
        {
            return Err(CliError::usage(format!(
                "guard `{name}` is not declared in this schema"
            )));
        }
        let secret = std::env::var(&self.secret_env).map_err(|_| {
            CliError::usage(format!(
                "guard `{name}`: required secret environment variable is missing"
            ))
        })?;
        HttpGuard::new(
            name.clone(),
            &self.endpoint,
            &secret,
            Duration::from_millis(self.deadline_ms),
            allow_loopback_http,
        )
        .map_err(|e| CliError::usage(format!("guard `{name}`: {e}")))
    }
}
