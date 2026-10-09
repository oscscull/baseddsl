//! Optional compiler-only `build.rs` integration. Never connects to a database.
//! Kept out of starters until the consumer build-cost gate accepts the overhead.
mod cache;
mod environment;
mod error;
mod inputs;
mod output;

pub use error::Error;
use std::path::PathBuf;

pub struct Generation {
    pub path: PathBuf,
    /// Whether the compiler checked BSL during this invocation (a cache miss).
    pub checked: bool,
    /// Whether the generated client bytes changed and were published.
    pub changed: bool,
}

/// Generate `OUT_DIR/client.rs` using the Cargo package's `based.toml`.
/// Prints Cargo input tracking; no CLI binary, rustfmt, runtime, or migrations.
pub fn generate() -> Result<Generation, Error> {
    let environment = environment::Environment::cargo()?;
    let inputs = inputs::Inputs::discover(&environment.root)?;
    inputs.track(&environment.root);
    let fingerprint = inputs.fingerprint()?;
    let path = environment.out.join("client.rs");
    println!("cargo:rerun-if-changed={}", path.display());
    let cache = cache::Cache::new(&environment.out);
    if cache.matches(&fingerprint, &path)? {
        return Ok(Generation {
            path,
            checked: false,
            changed: false,
        });
    }
    let checked = based_project::check_project(inputs.project)?;
    for warning in &checked.report.diagnostics {
        println!("cargo:warning={}: {}", warning.code, warning.message);
    }
    let bytes = output::client(&checked)?;
    let changed = output::publish(&path, bytes)?;
    cache.record(&fingerprint, &path)?;
    Ok(Generation {
        path,
        checked: true,
        changed,
    })
}
