//! Shared project + backend plumbing: the front end (`load_checked`), project discovery,
//! per-dialect backend construction, shard URL resolution, and the `facts` command.

use crate::error::{io_at, CliError};
use crate::render;
use based_ast::{Decl, FileId};
use based_codegen::Dialect;
use based_manifest::Project;
use based_sema::CheckedSchema;
use std::path::{Path, PathBuf};

/// The shard database URLs: the repeated `--database-url` flag wins, else the
/// comma-separated `BASED_DATABASE_URL`, else the ubiquitous single `DATABASE_URL` (the
/// convention the quickstarts + most hosting platforms use). Errors when none is set (a
/// live database is required to apply/status/serve).
pub fn shard_urls(database_url: Vec<String>) -> Result<Vec<String>, CliError> {
    let urls: Vec<String> = if !database_url.is_empty() {
        database_url
    } else {
        std::env::var("BASED_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .ok()
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default()
    };
    if urls.is_empty() {
        return Err(CliError::usage(
            "no database url: pass --database-url <url> (repeatable) or set BASED_DATABASE_URL / DATABASE_URL",
        ));
    }
    Ok(urls)
}

/// Build a single-shard [`based_runtime::Backend`] over `url` for the manifest dialect —
/// the same driver stack `based serve` uses (MariaDB/Postgres via a single-shard router;
/// SQLite over a file).
pub fn backend(dialect: Dialect, url: &str) -> Result<Box<dyn based_runtime::Backend>, CliError> {
    use based_runtime::driver::{PoolConfig, ShardRouter};

    let connecting = || format!("connecting to {}", redact(url));
    let backend: Box<dyn based_runtime::Backend> = match dialect {
        Dialect::MariaDb | Dialect::MySql => Box::new(
            ShardRouter::single(url, PoolConfig::default())
                .map_err(|e| CliError::db(connecting(), e))?,
        ),
        Dialect::Postgres => Box::new(
            based_runtime::PgRouter::single(url, PoolConfig::default())
                .map_err(|e| CliError::db(connecting(), e))?,
        ),
        // A SQLite `url` is a filesystem path (or `:memory:`, useless for a persisted apply).
        Dialect::Sqlite => Box::new(
            based_runtime::SqliteBackend::open(url)
                .map_err(|e| CliError::db(format!("opening {url}"), e))?,
        ),
    };
    Ok(backend)
}

/// Discover the project (manifest + files) without running the full front end — apply/status
/// only need the manifest dialect, and must work against an in-progress schema.
pub fn discover_project(root: &Path) -> Result<Project, CliError> {
    match based_manifest::discover(root) {
        Ok(p) => Ok(p),
        Err(diags) => {
            render::render(&diags, &[]);
            Err(CliError::summary(
                true,
                format!("could not load project at {} (see above)", root.display()),
            ))
        }
    }
}

/// Redact a database URL's password for logging (`mysql://user:pw@host` → `mysql://user@host`).
pub fn redact(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(s), Some(at)) if at > s => {
            let scheme = &url[..s + 3];
            let creds = &url[s + 3..at];
            let user = creds.split(':').next().unwrap_or(creds);
            format!("{scheme}{user}@{}", &url[at + 1..])
        }
        _ => url.to_string(),
    }
}

/// The front end's output: the project, the checked schema, the declaration set,
/// the file sources (indexed by `FileId`, for span -> line:col), and the count of
/// warnings emitted.
pub type Loaded = (
    Project,
    CheckedSchema,
    Vec<Decl>,
    Vec<(PathBuf, String)>,
    usize,
);

/// Shared front end: discover -> parse -> sema. Renders every diagnostic and bails
/// on any error (a clean schema is a precondition for codegen).
pub fn load_checked(root: &Path) -> Result<Loaded, CliError> {
    // 1. Discover the closed set of `.bsl` files under the manifest root.
    let project = discover_project(root)?;

    // 2. Read + parse each file. Sources are kept for diagnostic rendering; their
    //    index is the `FileId` the parser stamps onto spans.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(project.files.len());
    for f in &project.files {
        let src = std::fs::read_to_string(&f.path).map_err(|e| io_at("reading", &f.path, e))?;
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

    // 3. Expand `...Shape` spreads (composition) into concrete fields, so sema and
    //    codegen see a flat, spread-free body. Runs on a clean parse only.
    if errors == 0 {
        let diags = based_sema::expand_spreads(&mut all_decls);
        count(&diags, &mut errors, &mut warnings);
        render::render(&diags, &sources);
    }

    // 4. Semantic analysis over the whole declaration set (only if parsing + spread
    //    expansion were clean — sema assumes well-formed input).
    let mut schema = CheckedSchema::default();
    if errors == 0 {
        let (checked, diags) = based_sema::check(&all_decls);
        count(&diags, &mut errors, &mut warnings);
        render::render(&diags, &sources);
        // Target-specific checks: what can only be judged once the manifest's compile
        // target is known (an opaque `raw({…})` map missing it, an index access method
        // it lacks).
        let dialect = based_codegen::Dialect::parse(&project.manifest.dialect);
        let target = based_sema::check_target(&checked, dialect.name());
        count(&target, &mut errors, &mut warnings);
        render::render(&target, &sources);
        // Dialect-gated constructs codegen can't emit for this target (an ordered to-many
        // nest on MySQL) — rejected here, never emitted as invalid SQL.
        let nests = based_codegen::ordered_nest_diagnostics(&checked, &all_decls, dialect);
        count(&nests, &mut errors, &mut warnings);
        render::render(&nests, &sources);
        // FK-convention checks: the divergence-reason rule, judged against the manifest's
        // `foreign_keys` value (a decorator flipping FK presence against it needs a reason).
        let fk = based_sema::check_foreign_keys(
            &checked,
            based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys),
        );
        count(&fk, &mut errors, &mut warnings);
        render::render(&fk, &sources);
        schema = checked;
        // Resolve `id: Id` primary keys to the project's default generation strategy
        // (`[schema] id`) so codegen sees the concrete uuid/ulid/serial type.
        based_sema::resolve_pk_default(
            &mut schema,
            based_sema::PkStrategy::parse(&project.manifest.schema.id),
        );
    }

    let n = sources.len();
    if errors > 0 {
        // The diagnostics are already framed rustc-style on stderr; this is the summary.
        return Err(CliError::summary(
            true,
            format!("check failed: {errors} error(s), {warnings} warning(s) across {n} file(s)"),
        ));
    }
    Ok((project, schema, all_decls, sources, warnings))
}

/// `based facts`: surface the engine-derived facts — the inferred
/// inverse pairings and join-key indexes an editor would show as hints.
pub fn cmd_facts(root: &Path, json: bool) -> Result<(), CliError> {
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

fn count(diags: &[based_diagnostics::Diagnostic], errors: &mut usize, warnings: &mut usize) {
    for d in diags {
        match d.severity {
            based_diagnostics::Severity::Error => *errors += 1,
            based_diagnostics::Severity::Warning => *warnings += 1,
        }
    }
}
