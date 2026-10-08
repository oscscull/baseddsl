//! Require a reproducible library source pin for an embedded starter.
use crate::error::CliError;

pub fn require_known() -> Result<(), CliError> {
    let revision = based_version::COMMIT;
    if matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(CliError::usage(
        "this CLI has no Git source identity for a pinned embedded library; install a versioned CLI built from a Git checkout",
    ))
}
