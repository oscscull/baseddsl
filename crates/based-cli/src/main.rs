//! `based` — the compiler driver.
//!
//! Milestone 1 wires `based check`: discover `.bsl` files -> parse -> sema ->
//! render diagnostics. Codegen subcommands (`gen sql`, `gen client`) come later.

mod render;

use anyhow::{bail, Context};
use based_ast::FileId;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

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

fn cmd_check(root: &Path) -> anyhow::Result<()> {
    // 1. Discover the closed set of `.bsl` files under the manifest root.
    let project = match based_manifest::discover(root) {
        Ok(p) => p,
        Err(diags) => {
            render::render(&diags, &[]);
            bail!("could not load project at {}", root.display());
        }
    };

    // 2. Read + parse each file. Sources are kept for diagnostic rendering; their
    //    index is the `FileId` the parser stamps onto spans.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(project.files.len());
    for f in &project.files {
        let src = std::fs::read_to_string(&f.path)
            .with_context(|| format!("reading {}", f.path.display()))?;
        sources.push((f.path.clone(), src));
    }

    let mut all_decls = Vec::new();
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for (i, (_, src)) in sources.iter().enumerate() {
        match based_parser::parse_file(src, FileId(i as u32)) {
            Ok(sf) => all_decls.extend(sf.decls),
            Err(diags) => {
                count(&diags, &mut errors, &mut warnings);
                render::render(&diags, &sources);
            }
        }
    }

    // 3. Semantic analysis over the whole declaration set (only if parsing was
    //    clean — sema assumes well-formed input).
    if errors == 0 {
        let (_schema, diags) = based_sema::check(&all_decls);
        count(&diags, &mut errors, &mut warnings);
        render::render(&diags, &sources);
    }

    let n = sources.len();
    if errors > 0 {
        bail!("check failed: {errors} error(s), {warnings} warning(s) across {n} file(s)");
    }
    if warnings > 0 {
        println!("ok with warnings: {warnings} warning(s) across {n} file(s)");
    } else {
        println!("ok: {n} file(s) parsed clean");
    }
    Ok(())
}

fn count(diags: &[based_diagnostics::Diagnostic], errors: &mut usize, warnings: &mut usize) {
    for d in diags {
        match d.severity {
            based_diagnostics::Severity::Error => *errors += 1,
            based_diagnostics::Severity::Warning => *warnings += 1,
        }
    }
}
