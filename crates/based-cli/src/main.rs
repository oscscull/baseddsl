//! `based` — the compiler driver.
//!
//! `based check`: discover `.bsl` files -> parse -> sema -> render diagnostics.
//! `based gen sql`: the same front end, then emit SQL DDL from the checked schema.
//! (`gen client` comes later.)

mod render;

use anyhow::{bail, Context};
use based_ast::{Decl, FileId};
use based_codegen::Dialect;
use based_manifest::Project;
use based_sema::CheckedSchema;
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
    /// Generate target artifacts from the checked schema.
    Gen {
        #[command(subcommand)]
        target: GenTarget,
    },
}

#[derive(Subcommand)]
enum GenTarget {
    /// Emit SQL DDL (`CREATE TABLE …`) for the manifest dialect.
    Sql {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { root } => cmd_check(&root),
        Command::Gen { target } => match target {
            GenTarget::Sql { root, out } => cmd_gen_sql(&root, out.as_deref()),
        },
    }
}

fn cmd_check(root: &Path) -> anyhow::Result<()> {
    let (_project, schema, _decls, warnings) = load_checked(root)?;
    let n = schema.models.len();
    if warnings > 0 {
        println!("ok with warnings: {warnings} warning(s) across {n} model(s)");
    } else {
        println!("ok: {n} model(s) checked clean");
    }
    Ok(())
}

fn cmd_gen_sql(root: &Path, out: Option<&Path>) -> anyhow::Result<()> {
    let (project, schema, decls, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    // Schema DDL first, then the parameterized query templates (M3 read side).
    let mut sql = based_codegen::sql::ddl(&schema, dialect);
    if !schema.queries.is_empty() {
        sql.push_str(
            "\n\n-- ============================== queries ==============================\n",
        );
        sql.push_str(&based_codegen::sql::dml::dml(&schema, &decls, dialect));
    }
    if !schema.mutations.is_empty() {
        sql.push_str(
            "\n\n-- ============================= mutations =============================\n",
        );
        sql.push_str(&based_codegen::sql::mutations::mutations(
            &schema, &decls, dialect,
        ));
    }
    match out {
        Some(path) => {
            std::fs::write(path, &sql).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {} ({} models)", path.display(), schema.models.len());
        }
        None => print!("{sql}"),
    }
    Ok(())
}

/// Shared front end: discover -> parse -> sema. Renders every diagnostic and bails
/// on any error (a clean schema is a precondition for codegen). Returns the project,
/// the checked schema, and the warning count.
fn load_checked(root: &Path) -> anyhow::Result<(Project, CheckedSchema, Vec<Decl>, usize)> {
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
    let mut schema = CheckedSchema::default();
    if errors == 0 {
        let (checked, diags) = based_sema::check(&all_decls);
        count(&diags, &mut errors, &mut warnings);
        render::render(&diags, &sources);
        schema = checked;
    }

    let n = sources.len();
    if errors > 0 {
        bail!("check failed: {errors} error(s), {warnings} warning(s) across {n} file(s)");
    }
    Ok((project, schema, all_decls, warnings))
}

fn count(diags: &[based_diagnostics::Diagnostic], errors: &mut usize, warnings: &mut usize) {
    for d in diags {
        match d.severity {
            based_diagnostics::Severity::Error => *errors += 1,
            based_diagnostics::Severity::Warning => *warnings += 1,
        }
    }
}
