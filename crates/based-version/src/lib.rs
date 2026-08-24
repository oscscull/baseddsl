//! Compile-time build identity shared by the `based` and `based-lsp` binaries.
//!
//! `LONG` folds the git commit + build date into the version string so `--version`
//! distinguishes a fresh binary from a stale one on `PATH` — the recurring failure mode
//! where an editor or shell silently runs an out-of-date compiler. Compare `COMMIT`
//! against `git rev-parse HEAD` to tell whether a rebuild is due.

/// The crate semver from `Cargo.toml` (`CARGO_PKG_VERSION`).
pub const SEMVER: &str = env!("CARGO_PKG_VERSION");

/// The git commit the binary was built from, or `"unknown"` outside a git checkout.
pub const COMMIT: &str = env!("BASED_VERSION_COMMIT");

/// Full version line: `<semver> (<short-sha>[-dirty] <date>)`. What `--version` prints.
pub const LONG: &str = env!("BASED_VERSION_LONG");
