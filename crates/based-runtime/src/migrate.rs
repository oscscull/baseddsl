//! Migration apply + the `_based_migrations` ledger.
//!
//! The offline half of migrations lives in [`based_codegen::migrate`] (snapshot, diff,
//! per-dialect SQL render, content hash). This is the live half: it carries a real
//! database from one migration state to the next, driven through the same [`Db`] seam the
//! request path uses (so `based migrate apply` and `based serve` share one driver stack).
//!
//! ## Execution model — snapshot-authoritative
//! A migration's executable steps are re-derived as `diff(snapshot[N-1], snapshot[N])` from
//! the stored `schema.snap`s — the same authoritative model `based migrate render` uses,
//! so the SQL applied is exactly the SQL a reviewer read, with no separate `up.mig` text
//! parser to drift. The `up.mig` file is the human-readable review artifact and the tamper
//! anchor: its [`content_hash`](based_codegen::migrate::content_hash) is recorded in the
//! ledger, and an edit to an already-applied migration is a hard error (never a silent
//! re-apply).
//!
//! ## The ledger
//! [`ensure_ledger`] creates the engine-owned `_based_migrations` table (id + content-hash +
//! applied_at) on first use; [`apply`] inserts one row per applied migration inside that
//! migration's own transaction, so a crash mid-apply leaves no ledger row and a re-`apply`
//! retries cleanly. Destructive steps refuse to apply without an explicit
//! `--allow-destructive` ack.
//!
//! ## Rollback
//! Roll-forward is the default; there is no auto-generated down. An optional author-written
//! `down.mig` (raw per-dialect SQL) is honored by [`Direction::Down`] / [`Direction::To`],
//! each run inside a transaction that also deletes the ledger row. A migration with no
//! `down.mig` is roll-forward only ([`MigrateError::NoDown`]).

use std::collections::BTreeSet;
use std::path::Path;

use based_codegen::migrate::{self, Snapshot};
use based_codegen::Dialect;

use crate::run::{fetch_all, Backend, DbError, DbRead, Row};
use crate::value::SqlValue;

/// The engine-owned ledger table. Underscore-prefixed so it never collides with a user
/// model's table (models are `snake_case(ModelName)` — no leading underscore).
const LEDGER: &str = "_based_migrations";

/// A migration loaded from `migrations/NNNN_slug/`, ready to apply against a live database.
#[derive(Debug, Clone)]
pub struct PlannedMigration {
    /// The zero-padded ordinal (`2` for `0002_add_barcode`).
    pub number: u32,
    /// The directory name (`0002_add_barcode`) — the ledger's primary key.
    pub id: String,
    /// The executable up statements for the target dialect (snapshot-authoritative).
    pub up_sql: Vec<String>,
    /// The `up.mig` content hash — the ledger tamper guard.
    pub up_hash: String,
    /// Any up step is destructive (a drop / narrowing / new not-null-without-default /
    /// new unique) → apply requires `--allow-destructive`.
    pub destructive: bool,
    /// Teach-at-checkpoint messages: a drop-one-column/add-one-same-family-column pair on a
    /// table is ambiguous with a rename, so the destructive gate points at `@was` (D105).
    pub rename_hints: Vec<String>,
    /// The author-written `down.mig` (raw per-dialect SQL, split into statements), if present.
    pub down_sql: Option<Vec<String>>,
}

/// One recorded row of the `_based_migrations` ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub id: String,
    pub content_hash: String,
    pub applied_at: String,
}

/// Which way [`apply`] should move the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Apply every pending migration forward (the default).
    Up,
    /// Reconcile the applied set to exactly `{number ≤ N}` — roll forward pending up to `N`
    /// and roll back (via `down.mig`) anything applied above `N`. `To(0)` rolls back all.
    To(u32),
    /// Roll back only the most-recently-applied migration (via its `down.mig`).
    Down,
}

/// Options for [`apply`].
#[derive(Debug, Clone)]
pub struct ApplyOpts {
    /// Vouch for destructive steps at apply time (`--allow-destructive`).
    pub allow_destructive: bool,
    pub direction: Direction,
}

impl Default for ApplyOpts {
    fn default() -> Self {
        Self {
            allow_destructive: false,
            direction: Direction::Up,
        }
    }
}

/// What [`apply`] did: the ids applied forward and rolled back, in execution order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: Vec<String>,
    pub rolled_back: Vec<String>,
}

/// Why a migrate operation failed.
#[derive(Debug, Clone)]
pub enum MigrateError {
    /// The database itself failed (connection, permission, bad SQL against real data).
    Db(DbError),
    /// A filesystem error reading the `migrations/` tree.
    Io(String),
    /// A `schema.snap`/`up.mig` could not be parsed or rendered (corrupt artifact, or a step
    /// with no in-place SQL for the dialect — a SQLite `ALTER COLUMN` needing a raw rebuild).
    Artifact(String),
    /// An already-applied migration's `up.mig` hash no longer matches the ledger — it was
    /// edited after apply. Applied history is immutable; fix forward with a new migration.
    Tamper {
        id: String,
        applied: String,
        current: String,
    },
    /// A migration's structural `up.mig` lines diverge from the SQL its `schema.snap` chain
    /// implies — a hand-edit to a structural step, which apply re-derives from the snapshot
    /// and would otherwise silently ignore. Edit the schema and re-run `based migrate gen`,
    /// or (for SQL the neutral vocabulary can't express) use a `raw(<dialect>)` line.
    UpMigDrift { id: String },
    /// A pending migration is destructive and no `--allow-destructive` ack was given.
    Destructive { id: String },
    /// A rollback target has no `down.mig` — that migration is roll-forward only.
    NoDown { id: String },
    /// The ledger and the `migrations/` tree disagree in a way that can't be reconciled
    /// (an applied migration missing from disk, a gap in the sequence, a non-prefix apply set).
    Order(String),
}

impl From<DbError> for MigrateError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {}", e.message),
            Self::Io(m) => write!(f, "{m}"),
            Self::Artifact(m) => write!(f, "{m}"),
            Self::Tamper { id, applied, current } => write!(
                f,
                "migration `{id}` was edited after it was applied (ledger hash {applied}, current {current}); \
                 applied history is immutable — fix forward with a new migration"
            ),
            Self::UpMigDrift { id } => write!(
                f,
                "migration `{id}` has a structural up.mig line edited away from schema.snap; \
                 structural steps derive from schema.snap — edit the schema and re-run \
                 `based migrate gen`, or use a raw(<dialect>) line for SQL the steps can't express"
            ),
            Self::Destructive { id } => write!(
                f,
                "migration `{id}` has destructive step(s); re-run with --allow-destructive to apply"
            ),
            Self::NoDown { id } => {
                write!(f, "migration `{id}` has no down.mig — it is roll-forward only")
            }
            Self::Order(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for MigrateError {}

// ---------- loading migrations from disk ----------------------------------

/// Read `<root>/migrations/NNNN_slug/` into an ordered [`PlannedMigration`] list for the
/// given dialect. Each migration's steps are re-derived as `diff(prev snapshot, this
/// snapshot)` and rendered to executable SQL; the `up.mig` bytes are hashed for the ledger;
/// a `down.mig` (raw SQL) is split into statements if present. The numbers must be a
/// gap-free `1..N` sequence (zero-padded, sequential).
pub fn load_migrations(
    root: &Path,
    dialect: Dialect,
) -> Result<Vec<PlannedMigration>, MigrateError> {
    let dir = root.join("migrations");
    let mut dirs = migration_dirs(&dir)?;
    dirs.sort_by_key(|(n, ..)| *n);

    let mut out = Vec::with_capacity(dirs.len());
    let mut prev = Snapshot::default();
    for (i, (number, name, path)) in dirs.iter().enumerate() {
        // Gap-free sequential invariant: the i-th migration (0-based) must be number i+1.
        if *number != (i as u32) + 1 {
            return Err(MigrateError::Order(format!(
                "migration numbering has a gap or is out of order at `{name}` (expected {:04})",
                i + 1
            )));
        }
        let snap_text = read_file(&path.join("schema.snap"))?;
        let snap = Snapshot::parse(&snap_text)
            .map_err(|e| MigrateError::Artifact(format!("{name}/schema.snap: {e}")))?;
        let steps = migrate::diff_snapshots(&prev, &snap);
        let mut up_sql = migrate::sql_statements(&steps, dialect)
            .map_err(|e| MigrateError::Artifact(format!("{name}/up.mig: {e}")))?;

        let up_text = read_file(&path.join("up.mig"))?;
        // Structural steps are authoritative from `schema.snap`; a hand-edit to a structural
        // `up.mig` line would otherwise be silently ignored at apply. Refuse it instead.
        if !migrate::up_mig_matches_snapshot(&up_text, &steps) {
            return Err(MigrateError::UpMigDrift { id: name.clone() });
        }
        // `raw(<dialect>)` escape steps can't be re-derived from the snapshots (opaque
        // SQL) — recover them from the authored `up.mig` and layer them after the
        // structural steps for the matching target.
        let raw_steps = migrate::parse_raw_steps(&up_text);
        up_sql.extend(
            migrate::sql_statements(&raw_steps, dialect)
                .map_err(|e| MigrateError::Artifact(format!("{name}/up.mig: {e}")))?,
        );
        let up_hash = migrate::content_hash(&up_text);
        let destructive = steps.iter().any(based_codegen::migrate::Step::destructive);
        // The rename teach hint keys off this migration's own diff (prev → this snapshot),
        // so the destructive gate can point a drop+add at `@was` (D105).
        let rename_hints = migrate::rename_hints(&prev, &snap)
            .iter()
            .map(based_codegen::migrate::RenameHint::message)
            .collect();

        // A `down.mig` counts only if it carries an executable statement. `gen` prefills one
        // with `-- … is irreversible …` comment lines for steps it can't mechanically
        // reverse; an untouched all-comment placeholder splits to zero statements, so the
        // migration stays roll-forward only (a `--down` on it is a loud `NoDown`, not a
        // silent no-op) until the author completes it.
        let down_path = path.join("down.mig");
        let down_sql = if down_path.is_file() {
            let stmts = split_sql(&read_file(&down_path)?);
            (!stmts.is_empty()).then_some(stmts)
        } else {
            None
        };

        out.push(PlannedMigration {
            number: *number,
            id: name.clone(),
            up_sql,
            up_hash,
            destructive,
            rename_hints,
            down_sql,
        });
        prev = snap;
    }
    Ok(out)
}

/// The `migrations/NNNN_slug/` directories as `(number, dir_name, path)`. A non-conforming
/// entry (no `NNNN_` prefix) is ignored — only zero-padded sequential dirs order the ledger.
fn migration_dirs(dir: &Path) -> Result<Vec<(u32, String, std::path::PathBuf)>, MigrateError> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| MigrateError::Io(format!("reading {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| MigrateError::Io(e.to_string()))?;
        let ft = entry
            .file_type()
            .map_err(|e| MigrateError::Io(e.to_string()))?;
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((num, _)) = name.split_once('_') {
            if let Ok(n) = num.parse::<u32>() {
                out.push((n, name, entry.path()));
            }
        }
    }
    Ok(out)
}

fn read_file(path: &Path) -> Result<String, MigrateError> {
    std::fs::read_to_string(path)
        .map_err(|e| MigrateError::Io(format!("reading {}: {e}", path.display())))
}

/// Split a raw SQL script into individual statements on `;`, dropping `--`/`#` comment lines
/// and blank fragments. A `down.mig` is hand-written raw SQL, terminated by `;`.
fn split_sql(script: &str) -> Vec<String> {
    script
        .split(';')
        .map(|frag| {
            frag.lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("--") && !t.starts_with('#')
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------- the ledger -----------------------------------------------------

/// Create the `_based_migrations` ledger if it does not exist (idempotent — safe to call on
/// every apply/status). The engine owns this table; the author never writes it.
pub async fn ensure_ledger<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
) -> Result<(), DbError> {
    db.execute(&create_ledger_sql(dialect), &[])
        .await
        .map(|_| ())
}

fn create_ledger_sql(dialect: Dialect) -> String {
    // Portable column types per dialect: a text id + hash, and a timestamp the DB stamps.
    let (id_ty, hash_ty, ts_ty) = match dialect {
        Dialect::MariaDb => ("VARCHAR(255)", "VARCHAR(64)", "DATETIME"),
        Dialect::Postgres => ("TEXT", "TEXT", "TIMESTAMP"),
        Dialect::Sqlite => ("TEXT", "TEXT", "TEXT"),
    };
    format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (\n  \
           {id} {id_ty} NOT NULL PRIMARY KEY,\n  \
           {hash} {hash_ty} NOT NULL,\n  \
           {at} {ts_ty} NOT NULL\n)",
        tbl = dialect.quote(LEDGER),
        id = dialect.quote("id"),
        hash = dialect.quote("content_hash"),
        at = dialect.quote("applied_at"),
    )
}

/// Read the applied-migrations ledger, ordered by id (== apply order, since ids are
/// zero-padded). Assumes [`ensure_ledger`] has run.
pub async fn applied<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: Dialect,
) -> Result<Vec<LedgerRow>, DbError> {
    let sql = format!(
        "SELECT {id}, {hash}, {at} FROM {tbl} ORDER BY {id}",
        id = dialect.quote("id"),
        hash = dialect.quote("content_hash"),
        at = dialect.quote("applied_at"),
        tbl = dialect.quote(LEDGER),
    );
    let rows = fetch_all(db.fetch(&sql, &[])).await?;
    Ok(rows
        .iter()
        .map(|r| LedgerRow {
            id: str_field(r, "id"),
            content_hash: str_field(r, "content_hash"),
            applied_at: str_field(r, "applied_at"),
        })
        .collect())
}

/// A row field read as a string (text/uuid/timestamp all ride the wire as JSON strings).
fn str_field(r: &Row, key: &str) -> String {
    match r.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn placeholder(dialect: Dialect, n: usize) -> String {
    match dialect {
        // Postgres binds `$1, $2, …`; MariaDB/SQLite bind `?`.
        Dialect::Postgres => format!("${n}"),
        Dialect::MariaDb | Dialect::Sqlite => "?".to_string(),
    }
}

fn insert_ledger_sql(dialect: Dialect) -> String {
    format!(
        "INSERT INTO {tbl} ({id}, {hash}, {at}) VALUES ({p1}, {p2}, CURRENT_TIMESTAMP)",
        tbl = dialect.quote(LEDGER),
        id = dialect.quote("id"),
        hash = dialect.quote("content_hash"),
        at = dialect.quote("applied_at"),
        p1 = placeholder(dialect, 1),
        p2 = placeholder(dialect, 2),
    )
}

fn delete_ledger_sql(dialect: Dialect) -> String {
    format!(
        "DELETE FROM {tbl} WHERE {id} = {p1}",
        tbl = dialect.quote(LEDGER),
        id = dialect.quote("id"),
        p1 = placeholder(dialect, 1),
    )
}

// ---------- apply ----------------------------------------------------------

/// Apply (or roll back) migrations against a live database, reconciling the `_based_migrations`
/// ledger to the requested [`Direction`]. Each migration runs in its own transaction on a
/// fresh checkout with its ledger row. Enforces the tamper guard (an edited applied
/// migration is a hard error), the destructive-ack gate, and the contiguous-prefix ledger
/// invariant.
///
/// The `migrations` slice is the full ordered set from [`load_migrations`]; already-applied
/// ones are skipped (roll-forward) or reversed (rollback), so `apply` is safe to re-run.
pub async fn apply(
    backend: &dyn Backend,
    dialect: Dialect,
    migrations: &[PlannedMigration],
    opts: &ApplyOpts,
) -> Result<ApplyReport, MigrateError> {
    let ledger = {
        let mut db = backend.checkout("").await?;
        ensure_ledger(&mut *db, dialect).await?;
        applied(&mut *db, dialect).await?
    };
    let applied_ids: BTreeSet<&str> = ledger.iter().map(|r| r.id.as_str()).collect();

    // Tamper + existence guard: every applied migration must still be on disk with the same
    // up.mig hash it was applied with (applied history is immutable).
    for row in &ledger {
        match migrations.iter().find(|m| m.id == row.id) {
            Some(m) if m.up_hash != row.content_hash => {
                return Err(MigrateError::Tamper {
                    id: row.id.clone(),
                    applied: row.content_hash.clone(),
                    current: m.up_hash.clone(),
                });
            }
            None => {
                return Err(MigrateError::Order(format!(
                    "migration `{}` is recorded in the ledger but missing from disk",
                    row.id
                )));
            }
            _ => {}
        }
    }

    // Contiguous-prefix invariant: if migration K is applied, every migration below K must be
    // too (the ledger is a prefix of the sequence). Guards a corrupt/hand-edited ledger.
    let latest_applied = migrations
        .iter()
        .filter(|m| applied_ids.contains(m.id.as_str()))
        .map(|m| m.number)
        .max();
    if let Some(top) = latest_applied {
        for m in migrations.iter().filter(|m| m.number < top) {
            if !applied_ids.contains(m.id.as_str()) {
                return Err(MigrateError::Order(format!(
                    "migration `{}` is unapplied but a later migration is applied — \
                     the ledger must be a contiguous prefix",
                    m.id
                )));
            }
        }
    }

    // The highest migration number that should remain applied after this run.
    let keep_max: u32 = match opts.direction {
        Direction::Up => migrations.last().map_or(0, |m| m.number),
        Direction::To(n) => n,
        Direction::Down => match latest_applied {
            Some(top) => migrations
                .iter()
                .filter(|m| applied_ids.contains(m.id.as_str()) && m.number < top)
                .map(|m| m.number)
                .max()
                .unwrap_or(0),
            None => 0,
        },
    };

    let mut report = ApplyReport::default();

    // Rollback phase: applied migrations above keep_max, newest first (reverse order).
    let mut rollback: Vec<&PlannedMigration> = migrations
        .iter()
        .filter(|m| applied_ids.contains(m.id.as_str()) && m.number > keep_max)
        .collect();
    rollback.sort_by_key(|m| std::cmp::Reverse(m.number));
    for m in rollback {
        let down = m
            .down_sql
            .as_ref()
            .ok_or_else(|| MigrateError::NoDown { id: m.id.clone() })?;
        run_in_tx(backend, down, LedgerOp::Delete(&m.id), dialect).await?;
        report.rolled_back.push(m.id.clone());
    }

    // Forward phase: pending migrations up to keep_max, oldest first.
    let mut forward: Vec<&PlannedMigration> = migrations
        .iter()
        .filter(|m| !applied_ids.contains(m.id.as_str()) && m.number <= keep_max)
        .collect();
    forward.sort_by_key(|m| m.number);
    for m in forward {
        // Gate destructive steps before touching the database.
        if m.destructive && !opts.allow_destructive {
            return Err(MigrateError::Destructive { id: m.id.clone() });
        }
        run_in_tx(
            backend,
            &m.up_sql,
            LedgerOp::Insert {
                id: &m.id,
                hash: &m.up_hash,
            },
            dialect,
        )
        .await?;
        report.applied.push(m.id.clone());
    }

    Ok(report)
}

/// The ledger write a migration's transaction ends with.
enum LedgerOp<'a> {
    Insert { id: &'a str, hash: &'a str },
    Delete(&'a str),
}

/// Run a migration's statements + its ledger write under one engine-owned transaction on
/// a fresh checkout. If any statement fails, the dropped [`crate::run::Tx`] rolls back and
/// the error surfaces — a migration is all-or-nothing. (On MySQL/MariaDB, DDL implicitly
/// commits, so the tx is best-effort there; the ledger row is still written in the same
/// connection turn, and a re-apply skips completed migrations.)
async fn run_in_tx(
    backend: &dyn Backend,
    stmts: &[String],
    ledger: LedgerOp<'_>,
    dialect: Dialect,
) -> Result<(), MigrateError> {
    let db = backend.checkout("").await?;
    let mut tx = db.begin().await.map_err(MigrateError::Db)?;
    for s in stmts {
        tx.execute(s, &[]).await.map_err(MigrateError::Db)?;
    }
    let (sql, params) = match ledger {
        LedgerOp::Insert { id, hash } => (
            insert_ledger_sql(dialect),
            vec![
                SqlValue::Text(id.to_string()),
                SqlValue::Text(hash.to_string()),
            ],
        ),
        LedgerOp::Delete(id) => (
            delete_ledger_sql(dialect),
            vec![SqlValue::Text(id.to_string())],
        ),
    };
    tx.execute(&sql, &params).await.map_err(MigrateError::Db)?;
    tx.commit().await.map_err(MigrateError::Db)?;
    Ok(())
}

// ---------- status ---------------------------------------------------------

/// The applied/pending state of one migration, for `based migrate status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationState {
    /// Applied, and its current `up.mig` hash still matches the ledger.
    Applied,
    /// Applied, but the `up.mig` was edited after apply (the tamper case — loud).
    HashMismatch { applied: String, current: String },
    /// On disk but not yet in the ledger.
    Pending,
}

/// Pair each on-disk migration with its ledger state (pure — the CLI formats it). An applied
/// row with no matching migration on disk is reported separately by the caller via [`applied`].
pub fn status(
    migrations: &[PlannedMigration],
    ledger: &[LedgerRow],
) -> Vec<(String, MigrationState)> {
    migrations
        .iter()
        .map(|m| {
            let state = match ledger.iter().find(|r| r.id == m.id) {
                None => MigrationState::Pending,
                Some(r) if r.content_hash == m.up_hash => MigrationState::Applied,
                Some(r) => MigrationState::HashMismatch {
                    applied: r.content_hash.clone(),
                    current: m.up_hash.clone(),
                },
            };
            (m.id.clone(), state)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sql_drops_comments_and_blanks() {
        let script = "-- reverse\nDROP TABLE `widget`;\n# another\nDROP INDEX `x`;\n";
        assert_eq!(
            split_sql(script),
            vec![
                "DROP TABLE `widget`".to_string(),
                "DROP INDEX `x`".to_string()
            ]
        );
    }

    #[test]
    fn ledger_sql_binds_dialect_placeholders() {
        assert!(insert_ledger_sql(Dialect::Postgres).contains("VALUES ($1, $2, CURRENT_TIMESTAMP)"));
        assert!(insert_ledger_sql(Dialect::MariaDb).contains("VALUES (?, ?, CURRENT_TIMESTAMP)"));
        assert!(delete_ledger_sql(Dialect::Postgres)
            .contains("WHERE `id` = $1".replace('`', "\"").as_str()));
        assert!(delete_ledger_sql(Dialect::Sqlite).contains("WHERE `id` = ?"));
    }

    fn planned(number: u32, id: &str, hash: &str) -> PlannedMigration {
        PlannedMigration {
            number,
            id: id.to_string(),
            up_sql: vec![],
            up_hash: hash.to_string(),
            destructive: false,
            rename_hints: vec![],
            down_sql: None,
        }
    }

    #[test]
    fn load_migrations_carries_the_rename_teach_hint() {
        // A migration that drops one column and adds one same-family column (a rename
        // spelled as drop+add) is destructive AND carries the teach hint the destructive
        // gate surfaces (D105).
        let dir = std::env::temp_dir().join(format!("based-mig-hint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m1 = dir.join("migrations/0001_init");
        let m2 = dir.join("migrations/0002_relabel");
        std::fs::create_dir_all(&m1).unwrap();
        std::fs::create_dir_all(&m2).unwrap();
        // The `up.mig` must agree with its `schema.snap` (structural steps are
        // snapshot-authoritative), so derive each one from its snapshot rather than a
        // placeholder — else `load_migrations` refuses it as drift.
        let snap1 = "snapshot v1 dialect=neutral\n\ntable widget\n  column label text not_null\n";
        let snap2 = "snapshot v1 dialect=neutral\n\ntable widget\n  column title text not_null\n";
        let up_for = |prev_text: Option<&str>, now_text: &str| {
            let prev = prev_text
                .map(|t| Snapshot::parse(t).unwrap())
                .unwrap_or_default();
            let now = Snapshot::parse(now_text).unwrap();
            migrate::render_up(&migrate::diff_snapshots(&prev, &now))
        };
        std::fs::write(m1.join("schema.snap"), snap1).unwrap();
        std::fs::write(m1.join("up.mig"), up_for(None, snap1)).unwrap();
        std::fs::write(m2.join("schema.snap"), snap2).unwrap();
        std::fs::write(m2.join("up.mig"), up_for(Some(snap1), snap2)).unwrap();

        let migs = load_migrations(&dir, Dialect::Postgres).expect("load");
        assert_eq!(migs.len(), 2);
        assert!(migs[1].destructive, "drop column is destructive");
        assert_eq!(migs[1].rename_hints.len(), 1, "one rename hint");
        assert!(
            migs[1].rename_hints[0].contains("label") && migs[1].rename_hints[0].contains("title"),
            "{:?}",
            migs[1].rename_hints
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_flags_pending_applied_and_mismatch() {
        let migs = vec![
            planned(1, "0001_init", "aaaa"),
            planned(2, "0002_add", "bbbb"),
            planned(3, "0003_more", "cccc"),
        ];
        let ledger = vec![
            LedgerRow {
                id: "0001_init".into(),
                content_hash: "aaaa".into(),
                applied_at: "t".into(),
            },
            // 0002 was edited after apply → hash mismatch.
            LedgerRow {
                id: "0002_add".into(),
                content_hash: "OLD".into(),
                applied_at: "t".into(),
            },
        ];
        let s = status(&migs, &ledger);
        assert_eq!(s[0].1, MigrationState::Applied);
        assert!(matches!(s[1].1, MigrationState::HashMismatch { .. }));
        assert_eq!(s[2].1, MigrationState::Pending);
    }
}
