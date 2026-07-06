//! `based` — the compiler driver.
//!
//! `based check`: discover `.bsl` files -> parse -> sema -> render diagnostics.
//! `based gen sql`: the same front end, then emit SQL DDL from the checked schema.
//! `based gen client`: a typed Rust client module. `based gen openapi`: an OpenAPI 3.1
//! spec over the same wire (polyglot clients via `openapi-generator`, D23).

mod render;

use anyhow::{bail, Context};
use based_ast::{Decl, FileId};
use based_codegen::{client::ClientTarget, Dialect};
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
    /// Show the engine-derived facts (inferred inverses + indexes) — principle 8.
    Facts {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Emit machine-readable JSON instead of the human-readable listing.
        #[arg(long)]
        json: bool,
    },
    /// Generate + manage schema migrations (E2: `gen` — snapshot + diff, offline).
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },
    /// Serve the checked schema as a live RPC service (`POST /q|m/<name>`).
    Serve {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Address to bind the HTTP listener on.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
        /// A database URL per physical shard (repeat for a sharded fleet). Falls back
        /// to `BASED_DATABASE_URL` (comma-separated) when none is passed.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
        /// Worker threads (the per-process concurrency ceiling). Defaults to the pool
        /// ceiling so every worker can hold a connection.
        #[arg(long)]
        workers: Option<usize>,
        /// Warm connections kept per shard pool.
        #[arg(long, default_value_t = 4)]
        pool_min: usize,
        /// Max connections per shard pool (the per-box concurrency cap).
        #[arg(long, default_value_t = 32)]
        pool_max: usize,
    },
}

#[derive(Subcommand)]
enum MigrateAction {
    /// Diff the current `.bsl` against the latest `schema.snap` and write the next
    /// `migrations/NNNN_slug/{up.mig, schema.snap}`. No changes ⇒ writes nothing.
    /// Offline + deterministic — never touches a database (E2).
    Gen {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A short label for the migration slug (snake-cased). When omitted, the slug
        /// is derived from the change (`init` for the first, else `schema_update`).
        name: Option<String>,
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
    /// Emit a typed client module for the manifest client target.
    Client {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Emit an OpenAPI 3.1 spec for the wire — feed it to `openapi-generator` for a
    /// client in any language (polyglot via one contract, not N emitters; D23).
    Openapi {
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
            GenTarget::Client { root, out } => cmd_gen_client(&root, out.as_deref()),
            GenTarget::Openapi { root, out } => cmd_gen_openapi(&root, out.as_deref()),
        },
        Command::Migrate { action } => match action {
            MigrateAction::Gen { root, name } => cmd_migrate_gen(&root, name.as_deref()),
        },
        Command::Facts { root, json } => cmd_facts(&root, json),
        Command::Serve {
            root,
            listen,
            database_url,
            workers,
            pool_min,
            pool_max,
        } => cmd_serve(&root, &listen, database_url, workers, pool_min, pool_max),
    }
}

fn cmd_check(root: &Path) -> anyhow::Result<()> {
    let (_project, schema, _decls, _sources, warnings) = load_checked(root)?;
    let n = schema.models.len();
    if warnings > 0 {
        println!("ok with warnings: {warnings} warning(s) across {n} model(s)");
    } else {
        println!("ok: {n} model(s) checked clean");
    }
    Ok(())
}

fn cmd_gen_sql(root: &Path, out: Option<&Path>) -> anyhow::Result<()> {
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
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

fn cmd_gen_client(root: &Path, out: Option<&Path>) -> anyhow::Result<()> {
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let target = ClientTarget::parse(&project.manifest.client);
    let code = based_codegen::client::client(&schema, &decls, target);
    match out {
        Some(path) => {
            std::fs::write(path, &code).with_context(|| format!("writing {}", path.display()))?;
            let n = schema.queries.len() + schema.mutations.len();
            eprintln!("wrote {} ({n} callable(s))", path.display());
        }
        None => print!("{code}"),
    }
    Ok(())
}

fn cmd_gen_openapi(root: &Path, out: Option<&Path>) -> anyhow::Result<()> {
    let (_project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let doc = based_codegen::openapi::openapi(&schema, &decls);
    match out {
        Some(path) => {
            std::fs::write(path, &doc).with_context(|| format!("writing {}", path.display()))?;
            let n = schema.queries.len() + schema.mutations.len();
            eprintln!("wrote {} ({n} operation(s))", path.display());
        }
        None => print!("{doc}"),
    }
    Ok(())
}

/// `based migrate gen [name]`: diff the current `.bsl` against the latest captured
/// snapshot and, if there are changes, write the next `migrations/NNNN_slug/{up.mig,
/// schema.snap}` (E2). Offline + deterministic: the baseline is a stored snapshot, never
/// a database. No changes ⇒ writes nothing and says so (a clean exit).
fn cmd_migrate_gen(root: &Path, name: Option<&str>) -> anyhow::Result<()> {
    use based_codegen::migrate;

    let (_project, schema, _decls, _sources, _warnings) = load_checked(root)?;
    let migrations_dir = root.join("migrations");

    // The baseline is the highest-NNNN migration's snapshot (empty for 0001_init).
    let existing = existing_migrations(&migrations_dir)?;
    let prev = match existing.last() {
        Some((_, dir)) => {
            let snap_path = dir.join("schema.snap");
            let text = std::fs::read_to_string(&snap_path)
                .with_context(|| format!("reading {}", snap_path.display()))?;
            migrate::Snapshot::parse(&text)
                .with_context(|| format!("parsing {}", snap_path.display()))?
        }
        None => migrate::Snapshot::default(),
    };

    let steps = migrate::diff(&prev, &schema);
    if steps.is_empty() {
        println!("no schema changes since the latest migration — nothing to generate");
        return Ok(());
    }

    // Next number is a count of existing dirs (never a timestamp — determinism, E2).
    let next = existing.last().map(|(n, _)| n + 1).unwrap_or(1);
    let slug = migration_slug(name, next);
    let dir_name = format!("{next:04}_{slug}");
    let dir = migrations_dir.join(&dir_name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let up = migrate::render_up(&steps);
    let snap = migrate::snapshot(&schema);
    std::fs::write(dir.join("up.mig"), &up)?;
    std::fs::write(dir.join("schema.snap"), &snap)?;

    let destructive = steps.iter().filter(|s| s.destructive()).count();
    println!(
        "wrote migrations/{dir_name}/ ({} step(s){})",
        steps.len(),
        if destructive > 0 {
            format!(", {destructive} destructive")
        } else {
            String::new()
        }
    );
    Ok(())
}

/// Existing `migrations/NNNN_slug/` directories, sorted by their `NNNN` number. A
/// non-conforming entry (no `NNNN_` prefix) is ignored — only zero-padded sequential
/// dirs order the ledger (migrations.md).
fn existing_migrations(dir: &Path) -> anyhow::Result<Vec<(u32, PathBuf)>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((num, _)) = name.split_once('_') {
            if let Ok(n) = num.parse::<u32>() {
                out.push((n, entry.path()));
            }
        }
    }
    out.sort_by_key(|(n, _)| *n);
    Ok(out)
}

/// The migration slug: the snake-cased `name` argument, or a default (`init` for the
/// first migration, else `schema_update`). Cosmetic — only `NNNN` orders (migrations.md).
fn migration_slug(name: Option<&str>, number: u32) -> String {
    match name {
        Some(n) => based_sema::snake_case(&n.replace([' ', '-'], "_")),
        None if number == 1 => "init".to_string(),
        None => "schema_update".to_string(),
    }
}

/// The front end's output: the project, the checked schema, the declaration set,
/// the file sources (indexed by `FileId`, for span -> line:col), and the count of
/// warnings emitted.
type Loaded = (
    Project,
    CheckedSchema,
    Vec<Decl>,
    Vec<(PathBuf, String)>,
    usize,
);

/// Shared front end: discover -> parse -> sema. Renders every diagnostic and bails
/// on any error (a clean schema is a precondition for codegen).
fn load_checked(root: &Path) -> anyhow::Result<Loaded> {
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
    Ok((project, schema, all_decls, sources, warnings))
}

/// `based facts`: surface the engine-derived facts (principle 8) — the inferred
/// inverse pairings and join-key indexes an editor would show as hints.
fn cmd_facts(root: &Path, json: bool) -> anyhow::Result<()> {
    let (_project, schema, decls, sources, _warnings) = load_checked(root)?;
    let facts = based_facts::facts(&schema, &decls);
    if json {
        print!("{}", render::facts_json(&facts, &sources));
    } else if facts.is_empty() {
        println!("no derived facts");
    } else {
        print!("{}", render::facts_text(&facts, &sources));
    }
    Ok(())
}

/// `based serve`: stand the checked schema up as a live RPC service. Runs the same
/// front end as every other command (rendering diagnostics, bailing on any error —
/// a dirty schema never serves), builds the sharded connection pool, and hands both to
/// the runtime's HTTP listener. Blocks until the process is killed.
#[allow(clippy::too_many_arguments)]
fn cmd_serve(
    root: &Path,
    listen: &str,
    database_url: Vec<String>,
    workers: Option<usize>,
    pool_min: usize,
    pool_max: usize,
) -> anyhow::Result<()> {
    use based_runtime::driver::{PoolConfig, ShardRouter};
    use based_runtime::http::{serve_with_handle, ServeConfig, TrustedHeaderContext};
    use based_runtime::Compiled;

    // Shard URLs: the repeated flag wins; else BASED_DATABASE_URL (comma-separated).
    let urls: Vec<String> = if !database_url.is_empty() {
        database_url
    } else {
        std::env::var("BASED_DATABASE_URL")
            .ok()
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default()
    };
    if urls.is_empty() {
        bail!("no database url: pass --database-url <url> (repeatable) or set BASED_DATABASE_URL");
    }

    // Reuse the shared front end so diagnostics render exactly as `based check` does,
    // then build the served artifact from the clean schema (no second parse/check).
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let compiled = Compiled::from_checked(schema, decls, dialect);

    let pool = PoolConfig {
        min: pool_min,
        max: pool_max,
    };
    let router = ShardRouter::new(&urls, pool)
        .map_err(|e| anyhow::anyhow!("connecting to database: {}", e.message))?;
    // The shard key is schema-derived per callable (D33): the target model's `@scope`
    // owner field. No hand-set field here — the router reads it off the compiled schema.
    let ctx_source = TrustedHeaderContext;
    // Default workers to the pool ceiling so a worker never blocks waiting for a free
    // connection on a single shard (D20: bounded worker pool over the bounded conn pool).
    let config = ServeConfig {
        listen: listen.to_string(),
        workers: workers.unwrap_or(pool_max),
    };

    eprintln!(
        "based serve: {} shard(s), {} worker(s), listening on {listen}",
        router.shard_count(),
        config.workers,
    );
    eprintln!("liveness: GET /healthz  readiness: GET /readyz");

    // Graceful shutdown: on SIGTERM/SIGINT begin draining (readiness fails first, so a
    // load balancer pulls this instance out of rotation) and let in-flight requests
    // finish, then `serve_with_handle` returns. The handle is captured once the listener
    // is up (`on_start`), so the signal handler can only fire after we're serving.
    serve_with_handle(compiled, router, ctx_source, config, |handle| {
        if let Err(e) = ctrlc::set_handler(move || {
            eprintln!("based serve: shutdown signal received, draining…");
            handle.shutdown();
        }) {
            // A missing signal handler is non-fatal — the server still runs, it just
            // can't drain gracefully (a hard kill still stops it).
            eprintln!("based serve: could not install shutdown handler: {e}");
        }
    })
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn count(diags: &[based_diagnostics::Diagnostic], errors: &mut usize, warnings: &mut usize) {
    for d in diags {
        match d.severity {
            based_diagnostics::Severity::Error => *errors += 1,
            based_diagnostics::Severity::Warning => *warnings += 1,
        }
    }
}
