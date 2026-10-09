//! Parse the explicit starter mode and database choice.
use clap::{Args, ValueEnum};
use std::path::PathBuf;

#[derive(Args)]
pub struct Options {
    /// Empty destination directory; existing projects are never modified.
    #[arg(default_value = ".")]
    pub root: PathBuf,
    /// Choose an in-process Rust app or a standalone HTTP service.
    #[arg(long, value_enum)]
    pub mode: Mode,
    /// SQLite needs no database server. Other dialects require your own server.
    #[arg(long, value_enum, default_value = "sqlite")]
    pub dialect: Dialect,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    Embedded,
    Standalone,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Dialect {
    Sqlite,
    Mariadb,
    Postgres,
}

impl Dialect {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Mariadb => "mariadb",
            Self::Postgres => "postgres",
        }
    }
}
