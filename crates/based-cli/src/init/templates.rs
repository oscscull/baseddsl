//! Assemble starter source/configuration from the shared tutorial fixtures.
use super::options::{Dialect, Mode, Options};

pub fn files(options: &Options) -> Vec<(&'static str, String)> {
    let mut files = vec![
        (
            "schema/item.bsl",
            include_str!("../../starter/schema/item.bsl").into(),
        ),
        (
            ".gitignore",
            ".env\n*.db\n*.db-*\n*.db-journal\ntarget/\n__pycache__/\n".into(),
        ),
        (
            ".env.example",
            super::config::example(options.dialect).into(),
        ),
        ("README.md", super::instructions::readme(options)),
        ("based.toml", super::manifest::render(options)),
    ];
    match options.mode {
        Mode::Embedded => files.extend(embedded(options.dialect)),
        Mode::Standalone => files.push(("demo.py", include_str!("../../starter/demo.py").into())),
    }
    files
}

fn embedded(dialect: Dialect) -> Vec<(&'static str, String)> {
    let backend = match dialect {
        Dialect::Sqlite => include_str!("../../starter/embedded/sqlite.rs"),
        Dialect::Mariadb => include_str!("../../starter/embedded/mariadb.rs"),
        Dialect::Postgres => include_str!("../../starter/embedded/postgres.rs"),
    };
    vec![
        ("Cargo.toml", super::cargo::manifest(dialect)),
        (
            "src/main.rs",
            include_str!("../../starter/embedded/main.rs").into(),
        ),
        (
            "src/demo.rs",
            include_str!("../../starter/embedded/demo.rs").into(),
        ),
        (
            "src/session.rs",
            include_str!("../../starter/embedded/session.rs").into(),
        ),
        (
            "src/lookup.rs",
            include_str!("../../starter/embedded/lookup.rs").into(),
        ),
        ("src/database.rs", backend.into()),
    ]
}
