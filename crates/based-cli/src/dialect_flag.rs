//! The `--dialect` override shared by commands that render SQL offline.
use crate::error::CliError;
use based_codegen::Dialect;

/// The `--dialect` value when given, else the manifest's dialect.
pub fn resolve(flag: Option<&str>, manifest: &str) -> Result<Dialect, CliError> {
    match flag {
        Some(d) => Dialect::try_parse(d).ok_or_else(|| {
            CliError::usage(format!(
                "invalid --dialect {d:?}; expected mariadb | mysql | sqlite | postgres"
            ))
        }),
        None => Ok(Dialect::parse(manifest)),
    }
}
