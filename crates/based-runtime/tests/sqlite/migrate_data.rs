//! Migration data steps against a real SQLite engine: a chunked backfill commits batch by
//! batch and records the migration once done, and a literal-row `create` rolls back without
//! a `down.mig`.

use std::path::PathBuf;

use based_codegen::migrate::{diff_snapshots, render_down, render_up, IdKind, Snapshot};
use based_codegen::Dialect;
use based_runtime::fetch_all;
use based_runtime::migrate::{apply, load_migrations, ApplyOpts, Direction};
use based_runtime::run::Backend;
use based_runtime::sqlite::SqliteBackend;
use based_runtime::value::SqlValue;

const SNAP: &str = "snapshot v1 dialect=neutral

table card
  column name text not_null
  column paper bool not_null default=false

table card_printing
  column card_id uuid not_null fk=Card
  column digital bool not_null

table tag
  column name text not_null unique
";

/// Apply options with a deterministic id mint (uuid-shaped, so the length checks hold).
fn opts() -> ApplyOpts {
    fn mint(_: IdKind) -> String {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("00000000-0000-4000-8000-{n:012}")
    }
    ApplyOpts {
        mint: Some(mint),
        ..ApplyOpts::default()
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("based-data-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn migration(&self, name: &str, up: &str) {
        let dir = self.0.join("migrations").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("up.mig"), up).unwrap();
        std::fs::write(dir.join("schema.snap"), SNAP).unwrap();
    }

    fn down(&self, name: &str, down: &str) {
        std::fs::write(self.0.join("migrations").join(name).join("down.mig"), down).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn seeded(s: &Scratch) -> SqliteBackend {
    let snap = Snapshot::parse(SNAP).unwrap();
    s.migration(
        "0001_init",
        &render_up(&diff_snapshots(&Snapshot::default(), &snap)),
    );
    let backend = SqliteBackend::in_memory().unwrap();
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    apply(&backend, Dialect::Sqlite, &migs, &opts())
        .await
        .unwrap();
    let mut db = backend.checkout("").await.unwrap();
    for (id, name) in [("c1", "Bolt"), ("c2", "Arena Only"), ("c3", "Counter")] {
        db.execute(
            "INSERT INTO `card` (`id`, `name`) VALUES (?, ?)",
            &[SqlValue::Text(id.into()), SqlValue::Text(name.into())],
        )
        .await
        .unwrap();
    }
    for (id, card, digital) in [("p1", "c1", false), ("p2", "c2", true), ("p3", "c3", false)] {
        db.execute(
            "INSERT INTO `card_printing` (`id`, `card_id`, `digital`) VALUES (?, ?, ?)",
            &[
                SqlValue::Text(id.into()),
                SqlValue::Text(card.into()),
                SqlValue::Bool(digital),
            ],
        )
        .await
        .unwrap();
    }
    backend
}

async fn query(
    backend: &SqliteBackend,
    sql: &str,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let mut db = backend.checkout("").await.unwrap();
    fetch_all(db.fetch(sql, &[])).await.unwrap()
}

#[tokio::test]
async fn chunked_backfill_commits_in_batches_and_completes() {
    let s = Scratch::new("chunk");
    let backend = seeded(&s).await;
    s.migration(
        "0002_paper",
        "format 2\nupdate card where (card_printing.any(digital = false)) { paper = true } chunk 1\n",
    );
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    let report = apply(&backend, Dialect::Sqlite, &migs, &opts())
        .await
        .unwrap();
    assert_eq!(report.applied, vec!["0002_paper"]);

    let rows = query(&backend, "SELECT id FROM card WHERE paper = 1 ORDER BY id").await;
    let paper: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(paper, vec!["c1", "c3"]);
    let progress = query(
        &backend,
        "SELECT COUNT(*) AS n FROM _based_migration_progress",
    )
    .await;
    assert_eq!(
        progress[0]["n"].as_i64(),
        Some(0),
        "progress cleared once done"
    );
    let ledger = query(&backend, "SELECT id FROM _based_migrations ORDER BY id").await;
    assert_eq!(ledger.len(), 2);
}

#[tokio::test]
async fn literal_rows_roll_back_without_a_down_mig() {
    let s = Scratch::new("down");
    let backend = seeded(&s).await;
    s.migration(
        "0002_tags",
        "format 2\ncreate tag[] from [ { name = \"staple\" }, { name = \"combo\" } ]\n",
    );
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    apply(&backend, Dialect::Sqlite, &migs, &opts())
        .await
        .unwrap();
    let rows = query(&backend, "SELECT name FROM tag ORDER BY name").await;
    assert_eq!(rows.len(), 2);
    // Each created row got a freshly minted id.
    let ids = query(&backend, "SELECT id FROM tag").await;
    assert!(ids
        .iter()
        .all(|r| r["id"].as_str().is_some_and(|s| s.len() == 36)));

    let report = apply(
        &backend,
        Dialect::Sqlite,
        &migs,
        &ApplyOpts {
            direction: Direction::Down,
            ..opts()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.rolled_back, vec!["0002_tags"]);
    assert!(query(&backend, "SELECT name FROM tag").await.is_empty());
}

#[tokio::test]
async fn an_interrupted_chunked_step_resumes_from_its_cursor() {
    let s = Scratch::new("resume");
    let backend = seeded(&s).await;
    s.migration(
        "0002_paper",
        "format 2\nupdate all card { paper = true } chunk 1\n",
    );
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    // A run that committed the batch ending at `c1`, then stopped.
    {
        let mut db = backend.checkout("").await.unwrap();
        db.execute(
            "CREATE TABLE _based_migration_progress (id TEXT NOT NULL PRIMARY KEY, \
             content_hash TEXT NOT NULL, segment INTEGER NOT NULL, cursor TEXT NULL, \
             rows_done INTEGER NOT NULL)",
            &[],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO _based_migration_progress VALUES (?, ?, 0, ?, 1)",
            &[
                SqlValue::Text("0002_paper".into()),
                SqlValue::Text(migs[1].up_hash.clone()),
                SqlValue::Text(r#"["'c1'"]"#.into()),
            ],
        )
        .await
        .unwrap();
    }
    let report = apply(&backend, Dialect::Sqlite, &migs, &opts())
        .await
        .unwrap();
    assert_eq!(report.applied, vec!["0002_paper"]);
    // Only the rows after the cursor were written by the resumed run.
    let rows = query(&backend, "SELECT id FROM card WHERE paper = 1 ORDER BY id").await;
    let paper: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(paper, vec!["c2", "c3"]);
}

#[tokio::test]
async fn a_typed_down_mig_runs_its_data_steps() {
    let s = Scratch::new("typed-down");
    let backend = seeded(&s).await;
    s.migration("0002_paper", "format 2\nupdate all card { paper = true }\n");
    s.down(
        "0002_paper",
        "format 2\nupdate card where (name = \"Bolt\") { paper = false }\n",
    );
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    apply(&backend, Dialect::Sqlite, &migs, &opts())
        .await
        .unwrap();
    let report = apply(
        &backend,
        Dialect::Sqlite,
        &migs,
        &ApplyOpts {
            direction: Direction::Down,
            ..opts()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.rolled_back, vec!["0002_paper"]);
    let rows = query(&backend, "SELECT id FROM card WHERE paper = 1 ORDER BY id").await;
    let paper: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(paper, vec!["c2", "c3"]);
}

const ROUND_V1: &str = "snapshot v1 dialect=neutral

table round_widget
  column title text not_null
";

const ROUND_V2: &str = "snapshot v1 dialect=neutral

rename column round_widget.title -> name

table round_widget
  column name text not_null
  column size int null
  index idx_round_widget_size (size)
";

/// Drops the indexed `size` column: the index has to come off first.
const ROUND_V3: &str = "snapshot v1 dialect=neutral

table round_widget
  column name text not_null
";

/// Write the round-trip migrations (rename + add an indexed column, then drop it) with gen's
/// up and typed down.
fn round_trip_tree(dir: &std::path::Path) {
    let v1 = Snapshot::parse(ROUND_V1).unwrap();
    let v2 = Snapshot::parse(ROUND_V2).unwrap();
    let v3 = Snapshot::parse(ROUND_V3).unwrap();
    for (name, prev, now, snap) in [
        ("0001_init", Snapshot::default(), v1.clone(), ROUND_V1),
        ("0002_rename", v1, v2.clone(), ROUND_V2),
        ("0003_drop_size", v2, v3, ROUND_V3),
    ] {
        let d = dir.join("migrations").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("up.mig"), render_up(&diff_snapshots(&prev, &now))).unwrap();
        std::fs::write(d.join("down.mig"), render_down(&prev, &now)).unwrap();
        std::fs::write(d.join("schema.snap"), snap).unwrap();
    }
}

async fn columns(backend: &SqliteBackend) -> Vec<String> {
    query(
        backend,
        "SELECT name FROM pragma_table_info('round_widget') ORDER BY name",
    )
    .await
    .iter()
    .map(|r| r["name"].as_str().unwrap().to_string())
    .collect()
}

#[tokio::test]
async fn typed_down_round_trips_up_down_up() {
    let s = Scratch::new("round-trip");
    round_trip_tree(&s.0);
    let backend = SqliteBackend::in_memory().unwrap();
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    // 0003 drops a column, so the forward runs vouch for it.
    let up = ApplyOpts {
        allow_destructive: true,
        ..opts()
    };
    // Up drops the indexed `size` column (index first, so SQLite accepts it).
    apply(&backend, Dialect::Sqlite, &migs, &up).await.unwrap();
    assert_eq!(columns(&backend).await, ["id", "name"]);
    {
        let mut db = backend.checkout("").await.unwrap();
        db.execute(
            "INSERT INTO round_widget (id, name) VALUES ('w1', 'Gizmo')",
            &[],
        )
        .await
        .unwrap();
    }
    let down = ApplyOpts {
        direction: Direction::Down,
        ..opts()
    };
    let report = apply(&backend, Dialect::Sqlite, &migs, &down)
        .await
        .unwrap();
    assert_eq!(report.rolled_back, vec!["0003_drop_size"]);
    assert_eq!(columns(&backend).await, ["id", "name", "size"]);
    let report = apply(&backend, Dialect::Sqlite, &migs, &down)
        .await
        .unwrap();
    assert_eq!(report.rolled_back, vec!["0002_rename"]);
    assert_eq!(columns(&backend).await, ["id", "title"]);
    let titles = query(&backend, "SELECT title FROM round_widget").await;
    assert_eq!(
        titles[0]["title"].as_str(),
        Some("Gizmo"),
        "the rename kept the data"
    );

    let report = apply(&backend, Dialect::Sqlite, &migs, &up).await.unwrap();
    assert_eq!(report.applied, vec!["0002_rename", "0003_drop_size"]);
    assert_eq!(columns(&backend).await, ["id", "name"]);
}
