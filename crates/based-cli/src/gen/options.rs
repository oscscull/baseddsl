//! CLI-only generation overrides and publication policy.
use based_artifacts::Policy;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Args, Default)]
pub struct OutputOptions {
    /// Override the configured destination (relative to the manifest directory).
    #[arg(short, long)]
    pub out: Option<PathBuf>,
    /// Check configured output freshness without writing anything.
    #[arg(long, conflicts_with = "force")]
    pub check: bool,
    /// Explicitly replace a user-owned output; migrations remain protected.
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    Wire,
    Embedded,
}

#[derive(Subcommand)]
pub enum GenTarget {
    /// Generate SQL DDL and parameterized query/mutation templates.
    Sql {
        root: Option<PathBuf>,
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Generate a typed Rust client; configuration supplies its destination and mode.
    Client {
        root: Option<PathBuf>,
        #[command(flatten)]
        output: OutputOptions,
        #[arg(long, conflicts_with = "mode")]
        embedded: bool,
        /// Override generate.client_mode in based.toml.
        #[arg(long, value_enum)]
        mode: Option<Mode>,
    },
    /// Generate an OpenAPI 3.1 contract.
    Openapi {
        root: Option<PathBuf>,
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Generate every configured artifact after checking/staging the complete set.
    All {
        root: Option<PathBuf>,
        #[arg(long, conflicts_with = "force")]
        check: bool,
        #[arg(long)]
        force: bool,
    },
}

impl OutputOptions {
    pub fn policy(&self) -> Policy {
        if self.check {
            return Policy::Check;
        }
        Policy::Write {
            overwrite_user_owned: self.force,
        }
    }
}
