//! Explain the few steps needed to run a starter.
use super::options::{Dialect, Mode, Options};

pub fn commands(options: &Options) -> (&'static str, &'static str) {
    let migrate = match options.dialect {
        Dialect::Sqlite => "based migrate apply --database-url local.db",
        _ => "based migrate apply",
    };
    (migrate, "cargo run")
}

fn serve(options: &Options) -> &'static str {
    match options.dialect {
        Dialect::Sqlite => {
            "based serve --listen 127.0.0.1:9090 --database-url local.db --init-idempotency-table"
        }
        _ => "based serve --listen 127.0.0.1:9090 --init-idempotency-table",
    }
}

fn steps(options: &Options) -> Vec<&'static str> {
    let (migrate, run) = commands(options);
    match options.mode {
        Mode::Embedded => vec![migrate, run],
        Mode::Standalone => vec![migrate, serve(options), "node edge/edge.mjs"],
    }
}

pub fn print(options: &Options) {
    println!(
        "Initialized {} starter in {}.",
        options.dialect.name(),
        options.root.display()
    );
    println!("In that directory, run:");
    for (number, step) in steps(options).iter().enumerate() {
        println!("  {}. {step}", number + 1);
    }
    if !matches!(options.dialect, Dialect::Sqlite) {
        println!("First export DATABASE_URL for your database; see .env.example.");
    }
    match options.mode {
        Mode::Embedded => println!("Schema updates: README.md"),
        Mode::Standalone => {
            println!(
                "Run steps 2 and 3 in separate terminals. First call and schema updates: README.md"
            );
        }
    }
}

pub fn readme(options: &Options) -> String {
    let (migrate, run) = commands(options);
    format!("# Based starter\n\nRequires the matching Based CLI, Rust/Cargo, and platform C build tools.\nSQLite uses local.db. Other dialects need your database's DATABASE_URL; see .env.example.\n\n```sh\n{migrate}\n{run}\n```\n\nThe sample uses a fixed local owner identity. Authenticate context in your application.\nKeep credentials and database files out of source control.\n\nAfter editing schema/item.bsl:\n\n```sh\nbased migrate gen\nbased migrate verify\n{migrate}\n{run}\n```\n\nReview migrations before applying them. `cargo build` regenerates the typed client from the schema.\nCommit migration history and Cargo.lock.\n\nRead [the book](https://github.com/oscscull/baseddsl/blob/main/book/src/introduction.md)\nfor schema renames, connection configuration, and deployment.\n")
}

pub fn standalone_readme(options: &Options) -> String {
    include_str!("../../starter/standalone/README.md")
        .replace("MIGRATE", commands(options).0)
        .replace("SERVE", serve(options))
}
