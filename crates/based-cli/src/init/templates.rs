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
            ".env\n*.db\n*.db-*\n*.db-journal\ntarget/\n".into(),
        ),
        (
            ".env.example",
            super::config::example(options.dialect).into(),
        ),
        ("based.toml", super::manifest::render(options)),
    ];
    match options.mode {
        Mode::Embedded => {
            files.push(("README.md", super::instructions::readme(options)));
            files.extend(embedded(options.dialect));
        }
        Mode::Standalone => files.extend([
            ("README.md", super::instructions::standalone_readme(options)),
            (
                "edge/edge.mjs",
                include_str!("../../starter/standalone/edge/edge.mjs").into(),
            ),
            (
                "edge/identity.mjs",
                include_str!("../../starter/standalone/edge/identity.mjs").into(),
            ),
        ]),
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
            "build.rs",
            include_str!("../../starter/embedded/build.rs").into(),
        ),
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
