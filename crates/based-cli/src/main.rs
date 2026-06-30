//! `based` — the compiler driver.
//!
//! Milestone 1 wires `based check`: discover `.bsl` files -> parse -> sema ->
//! render diagnostics. Codegen subcommands (`gen sql`, `gen client`) come later.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "based", version, about = "based DSL compiler")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse + typecheck the project, print diagnostics.
    Check {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { root } => cmd_check(&root),
    }
}

fn cmd_check(root: &std::path::Path) -> anyhow::Result<()> {
    // TODO(parser-milestone): discover -> parse each file -> gather decls ->
    // sema::check -> render diagnostics with ariadne; exit nonzero on error.
    let _project = based_manifest::discover(root);
    anyhow::bail!("`based check` is not implemented yet (parser milestone)")
}
