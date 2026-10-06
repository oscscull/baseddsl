//! Validate explicit manifest choices before downstream parsers use defaults.
use crate::Manifest;
use based_diagnostics::Diagnostic;

pub(crate) fn manifest(manifest: &Manifest) -> Result<(), Vec<Diagnostic>> {
    let choices: [(&str, &str, &[&str]); 4] = [
        (
            "dialect",
            &manifest.dialect,
            &["mariadb", "mysql", "sqlite", "postgres", "postgresql"],
        ),
        ("client", &manifest.client, &["rust"]),
        (
            "schema.foreign_keys",
            &manifest.schema.foreign_keys,
            &["all", "none"],
        ),
        (
            "schema.id",
            &manifest.schema.id,
            &["uuid", "ulid", "serial"],
        ),
    ];
    let diagnostics: Vec<_> = choices
        .into_iter()
        .filter(|(_, value, allowed)| !allowed.contains(value))
        .map(|(key, value, allowed)| {
            Diagnostic::error(
                "E0011",
                format!(
                    "invalid based.toml {key} = {value:?}; expected {}",
                    allowed.join(" | ")
                ),
            )
        })
        .collect();
    if diagnostics.is_empty() {
        return Ok(());
    }
    Err(diagnostics)
}
