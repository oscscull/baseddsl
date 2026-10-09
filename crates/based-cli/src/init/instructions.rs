//! Explain the two steps needed to run a starter.
use super::options::{Dialect, Options};

pub fn commands(options: &Options) -> (&'static str, &'static str) {
    let migrate = match options.dialect {
        Dialect::Sqlite => "based migrate apply --database-url local.db",
        _ => "based migrate apply",
    };
    (migrate, "cargo run")
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
        println!("First export DATABASE_URL for your database; see .env.example.");
    }
    println!("Schema updates: README.md");
}

pub fn readme(options: &Options) -> String {
    let (migrate, run) = commands(options);
    format!("# Based starter\n\nRequires the matching Based CLI, Rust/Cargo, and platform C build tools.\nSQLite uses local.db. Other dialects need your database's DATABASE_URL; see .env.example.\n\n```sh\n{migrate}\n{run}\n```\n\nThe sample uses a fixed local owner identity. Authenticate context in your application.\nKeep credentials and database files out of source control.\n\nAfter editing schema/item.bsl:\n\n```sh\nbased gen all\nbased migrate gen\nbased migrate verify\n{migrate}\n{run}\n```\n\nReview migrations before applying them. Generation does not alter the database.\nCommit generated artifacts, migration history, and Cargo.lock.\n\nRead [the book](https://github.com/oscscull/baseddsl/blob/main/book/src/introduction.md)\nfor schema renames, connection configuration, and deployment.\n")
}
