//! Explain the starter's ordered next steps and explicit schema update cycle.
use super::options::{Dialect, Mode, Options};

pub fn commands(options: &Options) -> (&'static str, &'static str) {
    let migrate = match options.dialect {
        Dialect::Sqlite => "based migrate apply --database-url local.db",
        _ => "based migrate apply",
    };
    let run = match options.mode {
        Mode::Embedded => "cargo run",
        Mode::Standalone if cfg!(target_os = "windows") => "py -3 demo.py",
        Mode::Standalone => "python3 demo.py",
    };
    (migrate, run)
}

pub fn print(options: &Options) {
    let (migrate, run) = commands(options);
    println!(
        "Initialized {} starter in {}.",
        options.dialect.name(),
        options.root.display()
    );
    println!("In that directory, run:\n  1. {migrate}\n  2. {run}");
    if !matches!(options.dialect, Dialect::Sqlite) {
        println!("First export DATABASE_URL for your existing database; see .env.example. No server was provisioned.");
    }
    println!("Schema changes and production guidance: README.md");
}

pub fn readme(options: &Options) -> String {
    let (migrate, run) = commands(options);
    let prerequisite = match options.mode {
        Mode::Embedded => "Rust at the manifest's minimum, Cargo/Git, and platform C build tools",
        Mode::Standalone => "Python 3.9+ (standard library only); no Rust or npm build",
    };
    format!("# Based starter\n\nLocal learning project. Prerequisites: installed `based` and {prerequisite}.\nSQLite is a local file and requires no database server. Other dialects require\nyour own reachable database and an exported `DATABASE_URL`; see `.env.example`.\nThe Rust consumer and HTTP demo read the process environment, not `.env`.\n\n## Create and read\n\nRun these in this directory after initialization:\n\n```sh\n{migrate}\n{run}\n```\n\nBoth print `created:` and `read:` with the same production UUID. The HTTP demo\nstarts a loopback-only service with process-memory replay storage on an available port and stops it afterward.\nThe demo supplies a fixed host owner context directly; it is not an authentication edge.\n\n## Change the schema\n\nEdit `schema/item.bsl`, then run:\n\n```sh\nbased gen all\nbased migrate gen\nbased migrate verify\n{migrate}\n{run}\n```\n\nGeneration and `migrate gen` only write artifacts; they never apply a database\nmigration. Review `migrations/` before the separate apply step. Generated Rust\nlives in `generated/`, included explicitly by the consumer, outside ordinary\nCargo formatting. Use `cargo fmt --check` and `cargo clippy -- -D warnings` for\nan embedded app. There is no build script or compiler hidden in a Cargo build.\nCommit the generated client, migrations, and the consumer's Cargo.lock.\n\n## Configuration and production\n\nThe library revision is pinned to the CLI's source commit; update it deliberately\nalongside the CLI and regenerate clients when upgrading. `init` accepts only an\nempty directory and never integrates into or overwrites an existing Rust project.\nDo not commit `.env`, database files, or credentials. SQLite defaults to `local.db`.\nIDs use the production UUID generator, never a sequential test generator.\nBefore deploying a service, follow the [trusted-edge deployment guide](https://github.com/oscscull/baseddsl/blob/main/docs/standalone-deployment.md).\n")
}
