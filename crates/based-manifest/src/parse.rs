//! Parse and validate `based.toml` text, wherever it came from.
use crate::{validate, Manifest, MANIFEST_NAME};
use based_diagnostics::Diagnostic;

/// Parse manifest text and reject invalid explicit choices.
pub fn parse(text: &str) -> Result<Manifest, Vec<Diagnostic>> {
    let manifest: Manifest = toml::from_str(text).map_err(|e| {
        vec![Diagnostic::error(
            "E0011",
            format!("invalid {MANIFEST_NAME}: {e}"),
        )]
    })?;
    validate::manifest(&manifest)?;
    Ok(manifest)
}
