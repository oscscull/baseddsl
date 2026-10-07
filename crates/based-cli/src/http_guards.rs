//! Trusted callback configuration, resolved once before opening any database.
use crate::error::CliError;
use based_runtime::external_guard::HttpGuard;
use based_runtime::{Compiled, Guards};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(clap::Args, Debug)]
pub struct GuardOptions {
    /// Operator-owned TOML callback mappings (secrets are read from process environment).
    #[arg(long, env = "BASED_GUARD_CONFIG")]
    pub guard_config: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    guards: Vec<Mapping>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    name: String,
    endpoint: String,
    secret_env: String,
    ca_file: Option<PathBuf>,
    #[serde(default = "default_deadline")]
    timeout_ms: i64,
    #[serde(default)]
    allow_loopback_http: bool,
}

fn default_deadline() -> i64 {
    2000
}

impl GuardOptions {
    pub fn build(&self, compiled: &Compiled) -> Result<Guards, CliError> {
        let mut guards = Guards::new();
        let mut names = std::collections::HashSet::new();
        if let Some(path) = &self.guard_config {
            let content = std::fs::read_to_string(path)
                .map_err(|_| CliError::usage("cannot read guard configuration"))?;
            // TOML errors may quote source lines containing a secret: keep diagnostics fixed.
            let config: Config = toml::from_str(&content).map_err(|_| CliError::usage("invalid guard configuration: expected [[guards]] with name, endpoint, secret_env and optional timeout_ms/allow_loopback_http"))?;
            for mapping in config.guards {
                if !compiled
                    .declared_guards()
                    .any(|(_, name)| name == mapping.name)
                    || !names.insert(mapping.name.clone())
                {
                    return Err(CliError::usage(format!(
                        "unknown or duplicate guard mapping `{}`",
                        mapping.name
                    )));
                }
                let secret = std::env::var(&mapping.secret_env).map_err(|_| {
                    CliError::usage(format!(
                        "guard `{}`: bearer secret environment variable is unavailable",
                        mapping.name
                    ))
                })?;
                let timeout_ms = u64::try_from(mapping.timeout_ms).map_err(|_| {
                    CliError::usage(format!(
                        "guard `{}`: deadline must be 1–30000 milliseconds",
                        mapping.name
                    ))
                })?;
                let mut guard = HttpGuard::new(
                    &mapping.endpoint,
                    &secret,
                    Duration::from_millis(timeout_ms),
                    mapping.allow_loopback_http,
                )
                .map_err(|reason| CliError::usage(format!("guard `{}`: {reason}", mapping.name)))?;
                if let Some(ca_file) = mapping.ca_file {
                    let ca_path = path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(ca_file);
                    let pem = std::fs::read(ca_path).map_err(|_| {
                        CliError::usage(format!(
                            "guard `{}`: cannot read callback CA file",
                            mapping.name
                        ))
                    })?;
                    guard = guard.with_ca_pem(&pem).map_err(|reason| {
                        CliError::usage(format!("guard `{}`: {reason}", mapping.name))
                    })?;
                }
                guards = guard.register(guards, mapping.name);
            }
        }
        let missing = guards.missing_for(compiled);
        if !missing.is_empty() {
            return Err(CliError::usage(format!(
                "{}; provide --guard-config",
                based_runtime::GuardSetupError { missing }
            )));
        }
        Ok(guards)
    }
}
