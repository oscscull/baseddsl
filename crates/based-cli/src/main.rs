//! `based` — the compiler driver.
//!
//! `based check`: discover `.bsl` files -> parse -> sema -> render diagnostics.
//! `based gen sql`: the same front end, then emit SQL DDL from the checked schema.
//! `based gen client`: a typed Rust client module. `based gen openapi`: an OpenAPI 3.1
//! spec over the same wire (polyglot clients via `openapi-generator`).

mod error;
mod render;

use based_ast::{Decl, FileId};
use based_codegen::{client::ClientTarget, Dialect};
use based_manifest::Project;
use based_sema::CheckedSchema;
use clap::{Parser, Subcommand};
use error::{io_at, CliError};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
    /// Format the project's `.bsl` files in the canonical layout.
    Fmt {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Don't write; exit nonzero if any file is not already formatted.
        #[arg(long)]
        check: bool,
    },
    /// Generate target artifacts from the checked schema.
    Gen {
        #[command(subcommand)]
        target: GenTarget,
    },
    /// Show the engine-derived facts (inferred inverses + indexes).
    Facts {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Emit machine-readable JSON instead of the human-readable listing.
        #[arg(long)]
        json: bool,
    },
    /// Generate + manage schema migrations (snapshot + diff, offline).
    Migrate {
        #[command(subcommand)]
        action: MigrateAction,
    },
    /// Serve the checked schema as a live RPC service (`POST /q|m/<name>`).
    Serve {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Address to bind the HTTP listener on. `BASED_LISTEN` overrides the default —
        /// a container sets `0.0.0.0:8080` there so the port is reachable from outside.
        #[arg(long, env = "BASED_LISTEN", default_value = "127.0.0.1:8080")]
        listen: String,
        /// A database URL per physical shard (repeat for a sharded fleet). Falls back
        /// to `BASED_DATABASE_URL` (comma-separated) when none is passed.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
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
    /// Offline + deterministic — never touches a database.
    Gen {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A short label for the migration slug (snake-cased). When omitted, the slug
        /// is derived from the change (`init` for the first, else `schema_update`).
        name: Option<String>,
    },
    /// Render migrations' neutral `up.mig` steps to per-dialect SQL and print it — the
    /// review-the-SQL step. Offline: reads the stored `schema.snap`s, never a DB.
    Render {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A specific migration number (`NNNN`) to render. When omitted, renders every
        /// migration in order.
        #[arg(long)]
        number: Option<u32>,
        /// Override the target dialect (`mariadb`/`sqlite`/`postgres`). Defaults to the
        /// manifest dialect.
        #[arg(long)]
        dialect: Option<String>,
    },
    /// Apply pending migrations to a live database, each under one transaction with a
    /// `_based_migrations` ledger insert + tamper-hash check. Destructive steps require
    /// `--allow-destructive`. Applies to every `--database-url` (a sharded fleet migrates
    /// each shard).
    Apply {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// A database URL per physical shard (repeat for a sharded fleet). Falls back to
        /// `BASED_DATABASE_URL` (comma-separated) when none is passed.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
        /// Vouch for destructive steps (drops / narrowing / new not-null-without-default /
        /// new unique). Without it, apply stops before the first destructive migration.
        #[arg(long)]
        allow_destructive: bool,
        /// Reconcile the applied set to exactly migrations `≤ N`: roll forward up to `N`,
        /// roll back (via `down.mig`) anything applied above it. `--to 0` rolls back all.
        #[arg(long)]
        to: Option<u32>,
        /// Roll back only the most-recently-applied migration (via its `down.mig`).
        #[arg(long, conflicts_with = "to")]
        down: bool,
    },
    /// Show applied vs. pending migrations, flagging any hash mismatch (an edited applied
    /// migration). Reads the ledger from a live database.
    Status {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// The database to read the ledger from (first shard). Falls back to
        /// `BASED_DATABASE_URL`.
        #[arg(long = "database-url")]
        database_url: Vec<String>,
    },
    /// Offline CI gate: confirm each `up.mig` still matches its `schema.snap` (no hand-edit
    /// drift) and the latest snapshot matches the current `.bsl` (no uncaptured changes).
    Verify {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
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
        /// Also emit the in-process **embedded bridge**: an `Embedded` `Transport`
        /// over `based_runtime::Engine` plus `client::embedded(&engine)`, so an embedding
        /// build gets a working client with no hand-written bridge. The consuming crate
        /// must depend on based-runtime; a pure-wire client leaves this off.
        #[arg(long)]
        embedded: bool,
    },
    /// Emit an OpenAPI 3.1 spec for the wire — feed it to `openapi-generator` for a
    /// client in any language (polyglot via one contract, not N emitters).
    Openapi {
        /// Project root (holds based.toml). Defaults to the current directory.
        #[arg(default_value = ".")]
        root: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

// The binary owns the async runtime; front-end commands are sync and just run on it,
// execution commands (serve, migrate apply/status) await the runtime's futures.
#[tokio::main]
async fn main() -> ExitCode {
    // clap prints its own usage error + exits 2 before we get here; our commands return a
    // structured error so `main` can pick a clean message + exit class (2 usage, 1 failure).
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => e.report(),
    }
}

async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Check { root } => cmd_check(&root),
        Command::Fmt { root, check } => cmd_fmt(&root, check),
        Command::Gen { target } => match target {
            GenTarget::Sql { root, out } => cmd_gen_sql(&root, out.as_deref()),
            GenTarget::Client {
                root,
                out,
                embedded,
            } => cmd_gen_client(&root, out.as_deref(), embedded),
            GenTarget::Openapi { root, out } => cmd_gen_openapi(&root, out.as_deref()),
        },
        Command::Migrate { action } => match action {
            MigrateAction::Gen { root, name } => cmd_migrate_gen(&root, name.as_deref()),
            MigrateAction::Render {
                root,
                number,
                dialect,
            } => cmd_migrate_render(&root, number, dialect.as_deref()),
            MigrateAction::Apply {
                root,
                database_url,
                allow_destructive,
                to,
                down,
            } => cmd_migrate_apply(&root, database_url, allow_destructive, to, down).await,
            MigrateAction::Status { root, database_url } => {
                cmd_migrate_status(&root, database_url).await
            }
            MigrateAction::Verify { root } => cmd_migrate_verify(&root),
        },
        Command::Facts { root, json } => cmd_facts(&root, json),
        Command::Serve {
            root,
            listen,
            database_url,
            pool_min,
            pool_max,
        } => cmd_serve(&root, &listen, database_url, pool_min, pool_max).await,
    }
}

fn cmd_check(root: &Path) -> Result<(), CliError> {
    let (_project, schema, _decls, _sources, warnings) = load_checked(root)?;
    let n = schema.models.len();
    if warnings > 0 {
        println!("ok with warnings: {warnings} warning(s) across {n} model(s)");
    } else {
        println!("ok: {n} model(s) checked clean");
    }
    Ok(())
}

/// `based fmt [--check]`: rewrite every discovered `.bsl` file in the canonical layout.
/// Without `--check` it writes each changed file in place; with `--check` it writes
/// nothing and exits nonzero if any file is not already formatted. A file that doesn't
/// parse can't be formatted — its diagnostics are framed rustc-style and the run fails.
fn cmd_fmt(root: &Path, check: bool) -> Result<(), CliError> {
    let project = discover_project(root)?;

    let mut changed = 0usize;
    let mut unparsed = 0usize;
    for f in &project.files {
        let src = std::fs::read_to_string(&f.path).map_err(|e| io_at("reading", &f.path, e))?;
        match based_fmt::format_source(&src) {
            Ok(formatted) => {
                if formatted == src {
                    continue;
                }
                changed += 1;
                if check {
                    eprintln!("would reformat {}", f.path.display());
                } else {
                    std::fs::write(&f.path, &formatted)
                        .map_err(|e| io_at("writing", &f.path, e))?;
                    eprintln!("formatted {}", f.path.display());
                }
            }
            Err(diags) => {
                unparsed += 1;
                render::render(&diags, &[(f.path.clone(), src)]);
            }
        }
    }

    if unparsed > 0 {
        return Err(CliError::summary(
            false,
            format!("fmt failed: {unparsed} file(s) with parse errors (see above)"),
        ));
    }
    if check && changed > 0 {
        return Err(CliError::summary(
            false,
            format!("{changed} file(s) not formatted — run `based fmt`"),
        ));
    }
    let n = project.files.len();
    if changed == 0 {
        println!("ok: {n} file(s) already formatted");
    } else {
        println!("reformatted {changed} of {n} file(s)");
    }
    Ok(())
}

fn cmd_gen_sql(root: &Path, out: Option<&Path>) -> Result<(), CliError> {
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let fks = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
    // Schema DDL first, then the parameterized query templates.
    let mut sql = based_codegen::sql::ddl_with(&schema, dialect, fks);
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
            std::fs::write(path, &sql).map_err(|e| io_at("writing", path, e))?;
            eprintln!("wrote {} ({} models)", path.display(), schema.models.len());
        }
        None => print!("{sql}"),
    }
    Ok(())
}

fn cmd_gen_client(root: &Path, out: Option<&Path>, embedded: bool) -> Result<(), CliError> {
    use based_codegen::client::ClientOptions;
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let target = ClientTarget::parse(&project.manifest.client);
    let opts = ClientOptions { embedded };
    let code = based_codegen::client::client_with(&schema, &decls, target, opts);
    match out {
        Some(path) => {
            std::fs::write(path, &code).map_err(|e| io_at("writing", path, e))?;
            let n = schema.queries.len() + schema.mutations.len();
            eprintln!("wrote {} ({n} callable(s))", path.display());
        }
        None => print!("{code}"),
    }
    Ok(())
}

fn cmd_gen_openapi(root: &Path, out: Option<&Path>) -> Result<(), CliError> {
    let (_project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let doc = based_codegen::openapi::openapi(&schema, &decls);
    match out {
        Some(path) => {
            std::fs::write(path, &doc).map_err(|e| io_at("writing", path, e))?;
            let n = schema.queries.len() + schema.mutations.len();
            eprintln!("wrote {} ({n} operation(s))", path.display());
        }
        None => print!("{doc}"),
    }
    Ok(())
}

/// `based migrate gen [name]`: diff the current `.bsl` against the latest captured
/// snapshot and, if there are changes, write the next `migrations/NNNN_slug/{up.mig,
/// schema.snap}`. Offline + deterministic: the baseline is a stored snapshot, never
/// a database. No changes ⇒ writes nothing and says so (a clean exit).
fn cmd_migrate_gen(root: &Path, name: Option<&str>) -> Result<(), CliError> {
    use based_codegen::migrate;

    let (project, schema, decls, sources, _warnings) = load_checked(root)?;
    let migrations_dir = root.join("migrations");

    // The baseline is the highest-NNNN migration's snapshot (empty for 0001_init).
    let existing = existing_migrations(&migrations_dir)?;
    let prev = match existing.last() {
        Some((_, dir)) => {
            let snap_path = dir.join("schema.snap");
            let text =
                std::fs::read_to_string(&snap_path).map_err(|e| io_at("reading", &snap_path, e))?;
            migrate::Snapshot::parse(&text)
                .map_err(|e| CliError::failure(format!("parsing {}: {e}", snap_path.display())))?
        }
        None => migrate::Snapshot::default(),
    };

    // The current snapshot under the project FK convention, so an FK add/remove/change
    // diffs and lands in the migration.
    let fks = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
    let now = migrate::Snapshot::from_schema_with(&schema, fks);
    let steps = migrate::diff_snapshots(&prev, &now);
    if steps.is_empty() {
        println!("no schema changes since the latest migration — nothing to generate");
        return Ok(());
    }

    // Next number is a count of existing dirs (never a timestamp — determinism).
    let next = existing.last().map(|(n, _)| n + 1).unwrap_or(1);
    let slug = migration_slug(name, next);
    let dir_name = format!("{next:04}_{slug}");
    let dir = migrations_dir.join(&dir_name);
    std::fs::create_dir_all(&dir).map_err(|e| io_at("creating", &dir, e))?;

    let up = migrate::render_up(&steps);
    let snap = now.render();
    // Prefill a `down.mig` for the manifest dialect: real reverse SQL where the step is
    // mechanically reversible, a loud irreversible comment otherwise, so a reverse exists to
    // complete instead of being silently never written.
    let dialect = Dialect::parse(&project.manifest.dialect);
    let down = migrate::render_down(&steps, dialect);
    let up_path = dir.join("up.mig");
    let snap_path = dir.join("schema.snap");
    let down_path = dir.join("down.mig");
    std::fs::write(&up_path, &up).map_err(|e| io_at("writing", &up_path, e))?;
    std::fs::write(&snap_path, &snap).map_err(|e| io_at("writing", &snap_path, e))?;
    std::fs::write(&down_path, &down).map_err(|e| io_at("writing", &down_path, e))?;

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

    // Self-consume any `@was` this migration just captured: the rename now lives durably in
    // the migration ledger (schema.snap + the `rename` step), so the source hint is dead
    // weight. Only a directive whose `rename` step was actually emitted is removed — a
    // still-live or spent `@was` is never touched.
    let edits = migrate::spent_was_edits(&steps, &schema, &decls, &sources);
    if !edits.is_empty() {
        consume_spent_was(&sources, &edits, &dir_name)?;
    }

    // Teach-at-checkpoint: when this diff drops one column and adds one same-family column
    // on a table, it is ambiguous with a rename — point at `@was` so the gesture is
    // discoverable with zero prior knowledge (D105).
    for hint in migrate::rename_hints(&prev, &now) {
        println!("hint: {}", hint.message());
    }
    Ok(())
}

/// Apply the [`spent_was_edits`](based_codegen::migrate::spent_was_edits) removals to the
/// `.bsl` sources they touch and write each file back, logging every consumed `@was`. The
/// removal is surgical (just the directive), so the rest of each declaration is byte-clean.
fn consume_spent_was(
    sources: &[(PathBuf, String)],
    edits: &[based_codegen::migrate::SpentWas],
    dir_name: &str,
) -> Result<(), CliError> {
    use based_codegen::migrate;
    use std::collections::BTreeMap;

    let mut by_file: BTreeMap<usize, Vec<migrate::SpentWas>> = BTreeMap::new();
    for e in edits {
        by_file.entry(e.file).or_default().push(e.clone());
    }
    for (fid, file_edits) in by_file {
        let (path, src) = &sources[fid];
        let rewritten = migrate::apply_spent_was(src, &file_edits);
        std::fs::write(path, &rewritten).map_err(|e| io_at("writing", path, e))?;
    }
    for e in edits {
        println!(
            "removed spent {} (rename captured in migrations/{dir_name}/)",
            e.label
        );
    }
    Ok(())
}

/// `based migrate render [--number NNNN] [--dialect D]`: render stored migrations' neutral
/// steps to per-dialect SQL and print it — the review-the-SQL step. Fully offline: the
/// steps for migration NNNN are re-derived as `diff(snapshot[NNNN-1], snapshot[NNNN])` from
/// the stored `schema.snap`s (the snapshot-authoritative model, which `based migrate verify`
/// asserts equals the `up.mig`), so no `up.mig` parser is needed here.
/// The dialect defaults to the manifest's; `--dialect` overrides for a cross-target review.
fn cmd_migrate_render(
    root: &Path,
    number: Option<u32>,
    dialect: Option<&str>,
) -> Result<(), CliError> {
    use based_codegen::migrate;

    // Only the manifest dialect is needed — render reads stored artifacts, not the schema,
    // so it does not run the full front end (it works even against an in-progress schema).
    let project = discover_project(root)?;
    let dialect = match dialect {
        Some(d) => Dialect::parse(d),
        None => Dialect::parse(&project.manifest.dialect),
    };

    let migrations_dir = root.join("migrations");
    let existing = existing_migrations(&migrations_dir)?;
    if existing.is_empty() {
        return Err(CliError::usage(format!(
            "no migrations under {} — run `based migrate gen` first",
            migrations_dir.display()
        )));
    }
    if let Some(n) = number {
        if !existing.iter().any(|(m, _)| *m == n) {
            return Err(CliError::usage(format!(
                "migration {n:04} not found under {}",
                migrations_dir.display()
            )));
        }
    }

    let read_snap = |dir: &Path| -> Result<migrate::Snapshot, CliError> {
        let path = dir.join("schema.snap");
        let text = std::fs::read_to_string(&path).map_err(|e| io_at("reading", &path, e))?;
        migrate::Snapshot::parse(&text)
            .map_err(|e| CliError::failure(format!("parsing {}: {e}", path.display())))
    };

    for (idx, (n, dir)) in existing.iter().enumerate() {
        if let Some(want) = number {
            if *n != want {
                continue;
            }
        }
        // The predecessor snapshot (empty for the first migration) is this migration's
        // diff baseline; the delta between the two is exactly this migration's steps.
        let prev = if idx == 0 {
            migrate::Snapshot::default()
        } else {
            read_snap(&existing[idx - 1].1)?
        };
        let now = read_snap(dir)?;
        let mut steps = migrate::diff_snapshots(&prev, &now);
        let name = dir.file_name().map(|s| s.to_string_lossy().into_owned());
        // Structural steps are snapshot-authoritative; refuse to render SQL for a migration
        // whose structural `up.mig` lines were hand-edited away from `schema.snap` (else the
        // rendered SQL would silently ignore the edit). `raw(<dialect>)` escapes are authored
        // into `up.mig`, not derivable from the snapshots — append them so the rendered SQL
        // matches what `apply` runs.
        if let Ok(up_text) = std::fs::read_to_string(dir.join("up.mig")) {
            if !migrate::up_mig_matches_snapshot(&up_text, &steps) {
                return Err(CliError::failure(format!(
                    "migration {} has a structural up.mig line edited away from schema.snap; \
                     edit the schema and re-run `based migrate gen`, or use a raw(<dialect>) line",
                    name.as_deref().unwrap_or("?")
                )));
            }
            steps.extend(migrate::parse_raw_steps(&up_text));
        }

        println!("-- migrations/{}/up.mig", name.as_deref().unwrap_or("?"));
        print!("{}", migrate::render_sql(&steps, dialect));
        println!();
    }
    Ok(())
}

/// `based migrate apply`: apply pending migrations (or roll back) against a live database,
/// reconciling the `_based_migrations` ledger. Runs against every `--database-url` in
/// turn — a sharded fleet migrates each shard with the same migration set.
async fn cmd_migrate_apply(
    root: &Path,
    database_url: Vec<String>,
    allow_destructive: bool,
    to: Option<u32>,
    down: bool,
) -> Result<(), CliError> {
    use based_runtime::migrate;

    // Only the manifest dialect is needed to render each step to executable SQL; apply reads
    // stored artifacts, not the schema, so it works even against an in-progress `.bsl`.
    let project = discover_project(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let migrations = migrate::load_migrations(root, dialect)
        .map_err(|e| CliError::migrate("loading migrations", e))?;
    if migrations.is_empty() {
        println!(
            "no migrations under {}/migrations — run `based migrate gen` first",
            root.display()
        );
        return Ok(());
    }

    let direction = match (down, to) {
        (true, _) => migrate::Direction::Down,
        (false, Some(n)) => migrate::Direction::To(n),
        (false, None) => migrate::Direction::Up,
    };
    let opts = migrate::ApplyOpts {
        allow_destructive,
        direction,
    };

    let urls = shard_urls(database_url)?;
    for url in &urls {
        let backend = backend(dialect, url)?;
        match migrate::apply(&*backend, dialect, &migrations, &opts).await {
            Ok(report) => report_apply(&report, &redact(url)),
            Err(e) => {
                // At the destructive gate, teach `@was`: the refused migration may be a
                // rename spelled as a drop+add (D105).
                if let migrate::MigrateError::Destructive { id } = &e {
                    if let Some(m) = migrations.iter().find(|m| &m.id == id) {
                        for hint in &m.rename_hints {
                            eprintln!("hint: {hint}");
                        }
                    }
                }
                return Err(CliError::migrate(
                    format!("applying migrations to {}", redact(url)),
                    e,
                ));
            }
        }
    }
    Ok(())
}

/// `based migrate status`: read the ledger and show applied vs. pending migrations, flagging
/// any hash mismatch (an edited applied migration) or an applied row missing from disk.
async fn cmd_migrate_status(root: &Path, database_url: Vec<String>) -> Result<(), CliError> {
    use based_runtime::migrate::{self, MigrationState};

    let project = discover_project(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let migrations = migrate::load_migrations(root, dialect)
        .map_err(|e| CliError::migrate("loading migrations", e))?;

    // Status is about applied-vs-pending, so it needs the ledger (first shard suffices).
    let urls = shard_urls(database_url)?;
    let backend = backend(dialect, &urls[0])?;
    let mut db = backend
        .checkout("")
        .await
        .map_err(|e| CliError::db(format!("connecting to {}", redact(&urls[0])), e))?;
    migrate::ensure_ledger(&mut *db, dialect)
        .await
        .map_err(|e| CliError::db("reading the migration ledger", e))?;
    let ledger = migrate::applied(&mut *db, dialect)
        .await
        .map_err(|e| CliError::db("reading the migration ledger", e))?;

    let states = migrate::status(&migrations, &ledger);
    let (mut applied, mut pending, mut mismatched) = (0, 0, 0);
    for (id, state) in &states {
        let tag = match state {
            MigrationState::Applied => {
                applied += 1;
                "applied"
            }
            MigrationState::Pending => {
                pending += 1;
                "pending"
            }
            MigrationState::HashMismatch { .. } => {
                mismatched += 1;
                "HASH MISMATCH (edited after apply)"
            }
        };
        println!("  {id}  {tag}");
    }
    // A ledger row with no on-disk migration = deleted history (loud).
    for row in &ledger {
        if !migrations.iter().any(|m| m.id == row.id) {
            mismatched += 1;
            println!("  {}  MISSING FROM DISK (in ledger, no directory)", row.id);
        }
    }
    println!("{applied} applied, {pending} pending, {mismatched} problem(s)");
    if mismatched > 0 {
        return Err(CliError::summary(
            false,
            format!("migration ledger has {mismatched} problem(s) (see above)"),
        ));
    }
    Ok(())
}

/// `based migrate verify`: the offline CI gate. Confirms each `up.mig` still matches the steps
/// its `schema.snap` chain implies (no hand-edit drift) and the latest snapshot matches the
/// current `.bsl` (no uncaptured schema changes). Never touches a database.
fn cmd_migrate_verify(root: &Path) -> Result<(), CliError> {
    use based_codegen::migrate;

    let (project, schema, _decls, _sources, _warnings) = load_checked(root)?;
    let fks = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
    let existing = existing_migrations(&root.join("migrations"))?;

    let mut problems = 0usize;
    let mut prev = migrate::Snapshot::default();
    for (idx, (n, dir)) in existing.iter().enumerate() {
        let name = dir.file_name().map(|s| s.to_string_lossy().into_owned());
        let name = name.as_deref().unwrap_or("?");
        if *n != (idx as u32) + 1 {
            eprintln!("  {name}: number out of sequence (expected {:04})", idx + 1);
            problems += 1;
        }
        let snap_path = dir.join("schema.snap");
        let snap_text =
            std::fs::read_to_string(&snap_path).map_err(|e| io_at("reading", &snap_path, e))?;
        let snap = match migrate::Snapshot::parse(&snap_text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  {name}: {e}");
                problems += 1;
                continue;
            }
        };
        // The structural steps the snapshots imply must still match the stored `up.mig`
        // (byte-canonical, `raw` lines stripped). A `raw(<dialect>)` escape isn't derivable
        // from the snapshots (opaque SQL), so a raw-carrying migration is reported `partial`
        // (not offline-verifiable).
        let steps = migrate::diff_snapshots(&prev, &snap);
        let up_path = dir.join("up.mig");
        let stored =
            std::fs::read_to_string(&up_path).map_err(|e| io_at("reading", &up_path, e))?;
        if !migrate::up_mig_matches_snapshot(&stored, &steps) {
            eprintln!("  {name}: up.mig has drifted from schema.snap (re-run `based migrate gen`)");
            problems += 1;
        } else if migrate::has_raw_step(&stored) {
            println!("  {name}: partial (carries a raw step — not offline-verifiable)");
        }
        // `W0109`: a raw step that mutates a snapshot-*modeled* table makes the snapshot blind
        // to the change (a raw on a view/trigger/extension is safe). A warning, not a failure.
        for step in migrate::parse_raw_steps(&stored) {
            if let migrate::Step::Raw { sql, .. } = &step {
                let touched = migrate::raw_modeled_tables(sql, &snap);
                if !touched.is_empty() {
                    println!(
                        "  {name}: {} raw step touches modeled table(s) {} — the snapshot is blind to it",
                        based_sema::code::RAW_MIGRATION_MODELED,
                        touched.join(", ")
                    );
                }
            }
        }
        prev = snap;
    }

    // The latest snapshot must equal the current schema — else there are uncaptured
    // changes. Compared via the diff (not raw equality) so a spent `@was` — whose rename
    // is already captured — reads as no change even while it lingers in the `.bsl`.
    let current = migrate::Snapshot::from_schema_with(&schema, fks);
    if existing.is_empty() {
        if !current.tables.is_empty() {
            eprintln!("  no migrations yet — run `based migrate gen` to capture the schema");
            problems += 1;
        }
    } else if !migrate::diff_snapshots(&prev, &current).is_empty() {
        eprintln!("  schema has uncaptured changes not in any migration — run `based migrate gen`");
        problems += 1;
    }

    if problems > 0 {
        return Err(CliError::summary(
            false,
            format!(
                "verify failed: {problems} problem(s) across {} migration(s) (see above)",
                existing.len()
            ),
        ));
    }
    println!("ok: {} migration(s) verified", existing.len());
    Ok(())
}

/// The shard database URLs: the repeated `--database-url` flag wins, else the
/// comma-separated `BASED_DATABASE_URL`, else the ubiquitous single `DATABASE_URL` (the
/// convention the quickstarts + most hosting platforms use). Errors when none is set (a
/// live database is required to apply/status/serve).
fn shard_urls(database_url: Vec<String>) -> Result<Vec<String>, CliError> {
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
fn backend(dialect: Dialect, url: &str) -> Result<Box<dyn based_runtime::Backend>, CliError> {
    use based_runtime::driver::{PoolConfig, ShardRouter};

    let connecting = || format!("connecting to {}", redact(url));
    let backend: Box<dyn based_runtime::Backend> = match dialect {
        Dialect::MariaDb => Box::new(
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
fn discover_project(root: &Path) -> Result<Project, CliError> {
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

/// Print an apply/rollback report line.
fn report_apply(report: &based_runtime::ApplyReport, target: &str) {
    for id in &report.rolled_back {
        println!("  rolled back {id}");
    }
    for id in &report.applied {
        println!("  applied {id}");
    }
    if report.applied.is_empty() && report.rolled_back.is_empty() {
        println!("{target}: already up to date");
    } else {
        println!(
            "{target}: {} applied, {} rolled back",
            report.applied.len(),
            report.rolled_back.len()
        );
    }
}

/// Redact a database URL's password for logging (`mysql://user:pw@host` → `mysql://user@host`).
fn redact(url: &str) -> String {
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

/// Existing `migrations/NNNN_slug/` directories, sorted by their `NNNN` number. A
/// non-conforming entry (no `NNNN_` prefix) is ignored — only zero-padded sequential
/// dirs order the ledger.
fn existing_migrations(dir: &Path) -> Result<Vec<(u32, PathBuf)>, CliError> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir).map_err(|e| io_at("reading", dir, e))? {
        let entry = entry.map_err(|e| io_at("reading", dir, e))?;
        if !entry
            .file_type()
            .map_err(|e| io_at("reading", dir, e))?
            .is_dir()
        {
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
/// first migration, else `schema_update`). Cosmetic — only `NNNN` orders.
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
fn load_checked(root: &Path) -> Result<Loaded, CliError> {
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

    // 3. Semantic analysis over the whole declaration set (only if parsing was
    //    clean — sema assumes well-formed input).
    let mut schema = CheckedSchema::default();
    if errors == 0 {
        let (checked, diags) = based_sema::check(&all_decls);
        count(&diags, &mut errors, &mut warnings);
        render::render(&diags, &sources);
        // Target-specific checks: what can only be judged once the manifest's compile
        // target is known (an opaque `raw({…})` map missing it, an index access method
        // it lacks).
        let target = based_sema::check_target(
            &checked,
            based_codegen::Dialect::parse(&project.manifest.dialect).name(),
        );
        count(&target, &mut errors, &mut warnings);
        render::render(&target, &sources);
        // FK-convention checks: the divergence-reason rule, judged against the manifest's
        // `foreign_keys` value (a decorator flipping FK presence against it needs a reason).
        let fk = based_sema::check_foreign_keys(
            &checked,
            based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys),
        );
        count(&fk, &mut errors, &mut warnings);
        render::render(&fk, &sources);
        schema = checked;
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
fn cmd_facts(root: &Path, json: bool) -> Result<(), CliError> {
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
async fn cmd_serve(
    root: &Path,
    listen: &str,
    database_url: Vec<String>,
    pool_min: usize,
    pool_max: usize,
) -> Result<(), CliError> {
    use based_runtime::driver::{PoolConfig, ShardRouter};
    use based_runtime::http::{ServeConfig, TrustedHeaderContext};
    use based_runtime::Compiled;

    // Shard URLs: the repeated flag wins; else BASED_DATABASE_URL / DATABASE_URL.
    let urls = shard_urls(database_url)?;

    // Reuse the shared front end so diagnostics render exactly as `based check` does,
    // then build the served artifact from the clean schema (no second parse/check).
    let (project, schema, decls, _sources, _warnings) = load_checked(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let compiled = Compiled::from_checked(schema, decls, dialect);

    // Pool sizing from the flags; the hardening timeouts (checkout + statement) keep
    // their conservative defaults (a saturated pool → fast 503, a runaway query
    // aborted). The pool is also the concurrency ceiling — requests past it wait at
    // most the checkout timeout, then fail fast.
    let pool = PoolConfig {
        min: pool_min,
        max: pool_max,
        ..PoolConfig::default()
    };
    let config = ServeConfig {
        listen: listen.to_string(),
    };

    eprintln!("based serve: {dialect:?}, listening on {listen}");
    eprintln!("liveness: GET /healthz  readiness: GET /readyz");

    // Build the backend for the manifest dialect and stand the listener up. The `@scope`
    // owner field routes to a shard schema-side, so no shard key is hand-set here —
    // the driver reads it off the compiled schema. SQLite is a single local file (one url,
    // one shared database), so it neither shards nor pools.
    let ctx = TrustedHeaderContext;
    match dialect {
        Dialect::MariaDb => {
            let router = ShardRouter::new(&urls, pool)
                .map_err(|e| CliError::db("connecting to database", e))?;
            run_listener(compiled, router, ctx, config).await
        }
        Dialect::Postgres => {
            let router = based_runtime::PgRouter::new(&urls, pool)
                .map_err(|e| CliError::db("connecting to database", e))?;
            run_listener(compiled, router, ctx, config).await
        }
        Dialect::Sqlite => {
            if urls.len() > 1 {
                return Err(CliError::usage(format!(
                    "sqlite serves a single database file, got {} urls",
                    urls.len()
                )));
            }
            let backend = based_runtime::SqliteBackend::open(&urls[0])
                .map_err(|e| CliError::db(format!("opening {}", urls[0]), e))?;
            run_listener(compiled, backend, ctx, config).await
        }
    }
}

/// Stand the listener up over a concrete backend and block until the process is signalled.
/// Generic over the backend so each dialect's driver drops in without the listener naming
/// it. Graceful shutdown: on SIGTERM/SIGINT begin draining (readiness fails first, so a
/// load balancer pulls this instance out of rotation) and let in-flight requests finish,
/// then the call returns. The handle is captured once the listener is up (`on_start`), so
/// the signal handler can only fire after we're serving.
async fn run_listener(
    compiled: based_runtime::Compiled,
    backend: impl based_runtime::Backend + 'static,
    ctx: based_runtime::http::TrustedHeaderContext,
    config: based_runtime::http::ServeConfig,
) -> Result<(), CliError> {
    based_runtime::http::serve_with_handle(compiled, backend, ctx, config, |handle| {
        if let Err(e) = ctrlc::set_handler(move || {
            eprintln!("based serve: shutdown signal received, draining…");
            handle.shutdown();
        }) {
            // A missing signal handler is non-fatal — the server still runs, it just
            // can't drain gracefully (a hard kill still stops it).
            eprintln!("based serve: could not install shutdown handler: {e}");
        }
    })
    .await
    .map_err(|e| CliError::caused_by("serve failed", e))
}

fn count(diags: &[based_diagnostics::Diagnostic], errors: &mut usize, warnings: &mut usize) {
    for d in diags {
        match d.severity {
            based_diagnostics::Severity::Error => *errors += 1,
            based_diagnostics::Severity::Warning => *warnings += 1,
        }
    }
}
