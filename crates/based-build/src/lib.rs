//! Compiler-only `build.rs` client generation. Never connects to a database.
mod cache;
mod embed;
mod environment;
mod error;
mod inputs;
mod migrations;
mod output;
mod report;

pub use error::Error;
use std::path::PathBuf;

pub struct Generation {
    pub path: PathBuf,
    /// Whether the compiler checked BSL during this invocation (a cache miss).
    pub checked: bool,
    /// Whether the generated client bytes changed and were published.
    pub changed: bool,
}

/// Generate the client from `build.rs`; failures become Cargo build errors with
/// source-located diagnostics. Returns `None` after reporting a failure.
pub fn run() -> Option<Generation> {
    report::report(generate())
}

/// Generate `OUT_DIR/client.rs` and `OUT_DIR/schema.rs` using the Cargo package's `based.toml`.
/// When the package has a `migrations/` directory, it is embedded as `schema::MIGRATIONS`
/// and must pass `based migrate verify`. Prints Cargo input tracking; no CLI binary,
/// rustfmt, runtime, or database.
pub fn generate() -> Result<Generation, Error> {
    let environment = environment::Environment::cargo()?;
    let inputs = inputs::Inputs::discover(&environment.root)?;
    inputs.track(&environment.root);
    let migrations = inputs.migrations.as_deref().unwrap_or_default();
    embed::write(
        &inputs.project,
        migrations,
        &environment.root,
        &environment.out,
    )?;
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
    if inputs.migrations.is_some() {
        migrations::verify(&checked, migrations)?;
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
