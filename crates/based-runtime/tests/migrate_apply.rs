//! `based migrate apply` end-to-end against a **real** engine (SQLite, feature `sqlite`, E4).
//!
//! Infra-free proof of the apply engine: it writes a real `migrations/NNNN_slug/` tree to a
//! temp dir, loads it ([`load_migrations`]), and applies it against a live in-memory SQLite
//! `Db`/`Backend` — the same seam `based serve` uses. It covers the whole E4 surface: a fresh
//! apply + ledger, a re-apply no-op, `status`, a `down.mig` rollback, the tamper guard, and the
//! destructive-ack gate. The MariaDB twin (`migrate_apply_mariadb.rs`) proves the same against a
//! live server over Docker; this one runs in the normal `cargo test` gate with no daemon.

#![cfg(feature = "sqlite")]

use std::path::PathBuf;

use based_codegen::Dialect;
use based_runtime::migrate::{
    apply, load_migrations, status, ApplyOpts, Direction, MigrateError, MigrationState,
};
use based_runtime::run::Backend;
use based_runtime::sqlite::SqliteBackend;
use based_runtime::value::SqlValue;

/// A throwaway project dir under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let mut dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("based-apply-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// Write one migration's artifacts under `migrations/<name>/`.
    fn migration(&self, name: &str, up: &str, snap: &str, down: Option<&str>) {
        let dir = self.0.join("migrations").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("up.mig"), up).unwrap();
        std::fs::write(dir.join("schema.snap"), snap).unwrap();
        if let Some(d) = down {
            std::fs::write(dir.join("down.mig"), d).unwrap();
        }
    }

    fn up_path(&self, name: &str) -> PathBuf {
        self.0.join("migrations").join(name).join("up.mig")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const INIT_SNAP: &str =
    "snapshot v1 dialect=neutral\n\ntable widget\n  column name text not_null\n";
const INIT_UP: &str = "create table widget {\n  column name text not_null\n}\n";

const SIZE_SNAP: &str =
    "snapshot v1 dialect=neutral\n\ntable widget\n  column name text not_null\n  column size int null\n";
const SIZE_UP: &str = "add column widget.size int null\n";
const SIZE_DOWN: &str = "ALTER TABLE `widget` DROP COLUMN `size`;\n";

/// A backend + the two base migrations (0001 create widget, 0002 add nullable size w/ down).
fn scenario(tag: &str) -> (Scratch, SqliteBackend) {
    let s = Scratch::new(tag);
    s.migration("0001_init", INIT_UP, INIT_SNAP, None);
    s.migration("0002_add_size", SIZE_UP, SIZE_SNAP, Some(SIZE_DOWN));
    let backend = SqliteBackend::in_memory().unwrap();
    (s, backend)
}

fn count_ledger(backend: &SqliteBackend) -> i64 {
    let rows = backend
        .checkout("")
        .unwrap()
        .fetch("SELECT COUNT(*) AS c FROM _based_migrations", &[])
        .unwrap();
    rows[0]["c"].as_i64().unwrap()
}

fn has_size_column(backend: &SqliteBackend) -> bool {
    // PRAGMA table_info lists the columns; `size` is present only after 0002 applies.
    backend
        .checkout("")
        .unwrap()
        .fetch("SELECT name FROM pragma_table_info('widget')", &[])
        .unwrap()
        .iter()
        .any(|r| r["name"].as_str() == Some("size"))
}

#[test]
fn fresh_apply_creates_tables_and_ledger_then_re_apply_is_a_noop() {
    let (s, backend) = scenario("fresh");
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    assert_eq!(migs.len(), 2);

    let mut db = backend.checkout("").unwrap();
    let report = apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap();
    drop(db);
    assert_eq!(report.applied, vec!["0001_init", "0002_add_size"]);
    assert!(report.rolled_back.is_empty());

    // The schema is real: widget exists with the added `size` column, and both ledger rows landed.
    assert!(has_size_column(&backend));
    assert_eq!(count_ledger(&backend), 2);

    // A write against the migrated schema works.
    backend
        .checkout("")
        .unwrap()
        .execute(
            "INSERT INTO `widget` (`id`, `name`, `size`) VALUES (?, ?, ?)",
            &[
                SqlValue::Text("w1".into()),
                SqlValue::Text("bolt".into()),
                SqlValue::Int(7),
            ],
        )
        .unwrap();

    // Re-apply: nothing pending, nothing changes.
    let mut db = backend.checkout("").unwrap();
    let report = apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap();
    assert!(report.applied.is_empty() && report.rolled_back.is_empty());
    assert_eq!(count_ledger(&backend), 2);
}

#[test]
fn status_reports_pending_then_applied() {
    let (s, backend) = scenario("status");
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();

    // Before apply: the ledger doesn't exist yet, so both are pending.
    let mut db = backend.checkout("").unwrap();
    based_runtime::migrate::ensure_ledger(&mut *db, Dialect::Sqlite).unwrap();
    let ledger = based_runtime::migrate::applied(&mut *db, Dialect::Sqlite).unwrap();
    let before = status(&migs, &ledger);
    assert!(before.iter().all(|(_, st)| *st == MigrationState::Pending));

    apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap();
    let ledger = based_runtime::migrate::applied(&mut *db, Dialect::Sqlite).unwrap();
    let after = status(&migs, &ledger);
    assert!(after.iter().all(|(_, st)| *st == MigrationState::Applied));
}

#[test]
fn down_rolls_back_the_latest_and_can_re_apply() {
    let (s, backend) = scenario("down");
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();

    let mut db = backend.checkout("").unwrap();
    apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap();
    assert!(has_size_column(&backend));

    // Roll back just 0002 via its down.mig: the size column is gone, ledger drops to 1.
    let report = apply(
        &mut *db,
        Dialect::Sqlite,
        &migs,
        &ApplyOpts {
            allow_destructive: false,
            direction: Direction::Down,
        },
    )
    .unwrap();
    assert_eq!(report.rolled_back, vec!["0002_add_size"]);
    assert!(!has_size_column(&backend));
    assert_eq!(count_ledger(&backend), 1);

    // Roll forward again: 0002 re-applies cleanly.
    let report = apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap();
    assert_eq!(report.applied, vec!["0002_add_size"]);
    assert!(has_size_column(&backend));
}

#[test]
fn a_migration_edited_after_apply_is_a_tamper_error() {
    let (s, backend) = scenario("tamper");
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    let mut db = backend.checkout("").unwrap();
    apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap();

    // Edit an already-applied migration's up.mig — the content hash now diverges from the ledger.
    std::fs::write(
        s.up_path("0002_add_size"),
        "add column widget.size int not_null\n",
    )
    .unwrap();
    let tampered = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    let err = apply(&mut *db, Dialect::Sqlite, &tampered, &ApplyOpts::default()).unwrap_err();
    assert!(matches!(err, MigrateError::Tamper { .. }), "{err}");
}

#[test]
fn a_destructive_migration_needs_the_allow_flag() {
    let (s, backend) = scenario("destructive");
    // 0003 drops the `name` column — destructive (data loss).
    s.migration(
        "0003_drop_name",
        "drop column widget.name  # DESTRUCTIVE\n",
        "snapshot v1 dialect=neutral\n\ntable widget\n  column size int null\n",
        None,
    );
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    assert!(migs[2].destructive);

    let mut db = backend.checkout("").unwrap();
    // Without the ack, apply stops before the destructive migration.
    let err = apply(&mut *db, Dialect::Sqlite, &migs, &ApplyOpts::default()).unwrap_err();
    assert!(matches!(err, MigrateError::Destructive { .. }), "{err}");
    // 0001 + 0002 (the safe ones) still applied before hitting the gate.
    assert_eq!(count_ledger(&backend), 2);

    // With the explicit ack, the drop applies.
    apply(
        &mut *db,
        Dialect::Sqlite,
        &migs,
        &ApplyOpts {
            allow_destructive: true,
            direction: Direction::Up,
        },
    )
    .unwrap();
    assert_eq!(count_ledger(&backend), 3);
    assert!(!has_size_column_named(&backend, "name"));
}

fn has_size_column_named(backend: &SqliteBackend, col: &str) -> bool {
    backend
        .checkout("")
        .unwrap()
        .fetch("SELECT name FROM pragma_table_info('widget')", &[])
        .unwrap()
        .iter()
        .any(|r| r["name"].as_str() == Some(col))
}

#[test]
fn missing_dir_number_is_an_order_error() {
    let s = Scratch::new("gap");
    s.migration("0001_init", INIT_UP, INIT_SNAP, None);
    // 0003 with no 0002 → a gap in the sequence.
    s.migration("0003_add_size", SIZE_UP, SIZE_SNAP, None);
    let err = load_migrations(&s.0, Dialect::Sqlite).unwrap_err();
    assert!(matches!(err, MigrateError::Order(_)), "{err}");
}
