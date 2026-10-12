//! Migration data steps against a real SQLite engine: a chunked backfill commits batch by
//! batch and records the migration once done, and a literal-row `create` rolls back without
//! a `down.mig`.

use std::path::PathBuf;

use based_codegen::migrate::{diff_snapshots, render_up, Snapshot};
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
    apply(&backend, Dialect::Sqlite, &migs, &ApplyOpts::default())
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
    let report = apply(&backend, Dialect::Sqlite, &migs, &ApplyOpts::default())
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
    apply(&backend, Dialect::Sqlite, &migs, &ApplyOpts::default())
        .await
        .unwrap();
    let rows = query(&backend, "SELECT name FROM tag ORDER BY name").await;
    assert_eq!(rows.len(), 2);
    // Each created row got a fresh uuid id.
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
            ..ApplyOpts::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.rolled_back, vec!["0002_tags"]);
    assert!(query(&backend, "SELECT name FROM tag").await.is_empty());
}
