//! `based migrate gen|render|apply|status|verify`: generate, review, apply, inspect, and
//! offline-verify schema migrations (snapshot + diff, ledger-tracked).

use crate::error::{io_at, CliError};
use crate::local_config::shard_urls;
use crate::project::{backend, discover_project, load_checked, redact};
use based_codegen::Dialect;
use std::path::{Path, PathBuf};

/// `based migrate gen [name]`: diff the current `.bsl` against the latest captured
/// snapshot and, if there are changes, write the next `migrations/NNNN_slug/{up.mig,
/// schema.snap}`. Offline + deterministic: the baseline is a stored snapshot, never
/// a database. No changes ⇒ writes nothing and says so (a clean exit), unless `data` asks for
/// a migration to hold hand-written data steps.
pub fn cmd_migrate_gen(root: &Path, name: Option<&str>, data: bool) -> Result<(), CliError> {
    use based_codegen::migrate;

    let (project, schema, decls, sources, _warnings) = load_checked(root)?;
    let migrations_dir = root.join("migrations");

    // The baseline is the highest-NNNN migration's snapshot (empty for 0001_init).
    let existing = existing_migrations(&migrations_dir)?;
    let prev = match existing.last() {
        Some((_, dir)) => read_snapshot(dir)?,
        None => migrate::Snapshot::default(),
    };

    // The current snapshot under the project FK convention, so an FK add/remove/change
    // diffs and lands in the migration.
    let fks = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
    let now = migrate::Snapshot::from_schema_with(&schema, fks);
    let steps = migrate::diff_snapshots(&prev, &now);
    if steps.is_empty() && !data {
        println!("no schema changes since the latest migration — nothing to generate");
        return Ok(());
    }

    // Next number is a count of existing dirs (never a timestamp — determinism).
    let next = existing.last().map_or(1, |(n, _)| n + 1);
    let slug = migration_slug(name, next);
    let dir_name = format!("{next:04}_{slug}");
    let dir = migrations_dir.join(&dir_name);
    std::fs::create_dir_all(&dir).map_err(|e| io_at("creating", &dir, e))?;

    let up = migrate::render_up(&steps);
    let snap = now.render();
    let up_path = dir.join("up.mig");
    let snap_path = dir.join("schema.snap");
    std::fs::write(&up_path, &up).map_err(|e| io_at("writing", &up_path, e))?;
    std::fs::write(&snap_path, &snap).map_err(|e| io_at("writing", &snap_path, e))?;
    // The typed reverse: the structural steps back to the prior snapshot, with room for
    // data steps. A data-only migration's down holds no step until one is written (its
    // literal-row creates reverse on their own).
    let down = migrate::render_down(&prev, &now);
    let down_path = dir.join("down.mig");
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
    // discoverable with zero prior knowledge.
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
pub fn cmd_migrate_render(
    root: &Path,
    number: Option<u32>,
    dialect: Option<&str>,
) -> Result<(), CliError> {
    use based_codegen::migrate;

    // Only the manifest dialect is needed — render reads stored artifacts, not the schema,
    // so it does not run the full front end (it works even against an in-progress schema).
    let project = discover_project(root)?;
    let dialect = crate::dialect_flag::resolve(dialect, &project.manifest.dialect)?;

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
            read_snapshot(&existing[idx - 1].1)?
        };
        let now = read_snapshot(dir)?;
        let name = dir.file_name().map(|s| s.to_string_lossy().into_owned());
        let name = name.as_deref().unwrap_or("?");
        // Structural steps are snapshot-authoritative; refuse to render SQL for a migration
        // whose structural `up.mig` lines were hand-edited away from `schema.snap` (else the
        // rendered SQL would silently ignore the edit). `raw(<dialect>)` escapes and data
        // steps are authored into `up.mig` — the plan places them as `apply` runs them.
        let up_text = std::fs::read_to_string(dir.join("up.mig")).unwrap_or_default();
        let steps = migrate::up_steps(&prev, &now, &up_text);
        if !migrate::up_mig_matches_snapshot(&up_text, &steps) {
            return Err(CliError::failure(format!(
                "migration {name} has a structural up.mig line edited away from schema.snap; \
                 edit the schema and re-run `based migrate gen`, or use a raw(<dialect>) line"
            )));
        }
        let plan = migrate::plan_up(&prev, &now, &up_text, dialect).map_err(|problems| {
            CliError::failure(format!(
                "migration {name}: {}",
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        })?;

        println!("-- migrations/{name}/up.mig");
        print!("{}", plan.review);
        println!();
    }
    Ok(())
}

/// `based migrate apply`: apply pending migrations (or roll back) against a live database,
/// reconciling the `_based_migrations` ledger. Runs against every `--database-url` in
/// turn — a sharded fleet migrates each shard with the same migration set.
pub async fn cmd_migrate_apply(
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
        progress: Some(report_chunk),
        mint: Some(migrate::production_mint),
    };

    let urls = shard_urls(root, dialect, database_url)?;
    for url in &urls {
        let backend = backend(dialect, url)?;
        match migrate::apply(&*backend, dialect, &migrations, &opts).await {
            Ok(report) => report_apply(&report, &redact(url)),
            Err(e) => {
                // At the destructive gate, surface the partial state: which migrations were
                // applied before it stopped (so the operator sees the DB is partially
                // migrated, not that nothing happened), and teach `@was` — the refused
                // migration may be a rename spelled as a drop+add.
                if let migrate::MigrateError::Destructive { id, applied } = &e {
                    for aid in applied {
                        println!("  applied {aid}");
                    }
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
pub async fn cmd_migrate_status(root: &Path, database_url: Vec<String>) -> Result<(), CliError> {
    use based_runtime::migrate::{self, MigrationState};

    let project = discover_project(root)?;
    let dialect = Dialect::parse(&project.manifest.dialect);
    let migrations = migrate::load_migrations(root, dialect)
        .map_err(|e| CliError::migrate("loading migrations", e))?;

    // Status is about applied-vs-pending, so it needs the ledger (first shard suffices).
    let urls = shard_urls(root, dialect, database_url)?;
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
pub fn cmd_migrate_verify(root: &Path) -> Result<(), CliError> {
    use based_project::Severity;

    let (project, schema, _decls, _sources, _warnings) = load_checked(root)?;
    let migrations = based_project::read_migrations(root).map_err(|e| match e {
        based_project::Error::Io { path, source } => io_at("reading", &path, source),
        other @ based_project::Error::Diagnostics(_) => CliError::failure(other.to_string()),
    })?;
    let sources: Vec<_> = migrations
        .iter()
        .map(based_project::MigrationFiles::source)
        .collect();
    let findings = based_project::verify_migrations(&schema, &project.manifest, &sources);

    let mut problems = 0usize;
    for finding in &findings {
        match finding.severity {
            Severity::Problem => {
                problems += 1;
                eprintln!("  {finding}");
            }
            Severity::Warning | Severity::Note => println!("  {finding}"),
        }
    }
    if problems > 0 {
        return Err(CliError::summary(
            false,
            format!(
                "verify failed: {problems} problem(s) across {} migration(s) (see above)",
                migrations.len()
            ),
        ));
    }
    println!("ok: {} migration(s) verified", migrations.len());
    Ok(())
}

/// Read + parse a migration directory's `schema.snap`.
fn read_snapshot(dir: &Path) -> Result<based_codegen::migrate::Snapshot, CliError> {
    let path = dir.join("schema.snap");
    let text = std::fs::read_to_string(&path).map_err(|e| io_at("reading", &path, e))?;
    based_codegen::migrate::Snapshot::parse(&text)
        .map_err(|e| CliError::failure(format!("parsing {}: {e}", path.display())))
}

/// Print one committed batch of a chunked data step.
fn report_chunk(p: &based_runtime::migrate::ChunkProgress) {
    println!(
        "  {}: {}: {} rows (chunk {})",
        p.migration, p.table, p.rows, p.chunk
    );
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
