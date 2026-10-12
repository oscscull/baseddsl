//! Migration data steps against **real** MariaDB and Postgres servers, over Docker: a JSON
//! field copy, a reference-row upsert, the paper backfill through `.any` (chunked), and a
//! chunked step resuming from a saved cursor; then an up → down → up round trip through
//! gen's typed down. Each test skips cleanly without a daemon.

#![cfg(feature = "docker-tests")]

#[path = "support/docker_mariadb.rs"]
mod docker_mariadb;
#[path = "support/docker_postgres.rs"]
mod docker_postgres;

use std::path::PathBuf;

use based_codegen::migrate::{diff_snapshots, render_down, render_up, Snapshot};
use based_codegen::Dialect;
use based_runtime::driver::{PoolConfig, ShardRouter};
use based_runtime::fetch_all;
use based_runtime::migrate::{apply, load_migrations, ApplyOpts};
use based_runtime::run::Backend;
use based_runtime::value::SqlValue;
use based_runtime::PgRouter;

use docker_mariadb::MariaDbContainer;
use docker_postgres::PostgresContainer;

const BEFORE: &str = "snapshot v1 dialect=neutral

table mig_card
  column name text not_null
  column paper bool not_null default=false

table mig_card_printing
  column card_id uuid not_null fk=MigCard
  column data json not_null
  column digital bool not_null

table mig_format
  column key text not_null unique
  column name text not_null
";

fn after() -> String {
    BEFORE.replace(
        "  column digital bool not_null\n",
        "  column digital bool not_null\n  column set_code text null\n",
    )
}

const BACKFILL: &str = r#"format 2
add column mig_card_printing.set_code text null
update all mig_card_printing { set_code = data.set }
update mig_card where (mig_card_printing.any(digital = false)) { paper = true } chunk 1
create mig_format[] from [
  { key = "commander", name = "Commander" },
  { key = "brawl", name = "Brawl" },
] on conflict (key) update { ...incoming }
"#;

const C1: &str = "00000000-0000-4000-8000-000000000001";
const C2: &str = "00000000-0000-4000-8000-000000000002";
const C3: &str = "00000000-0000-4000-8000-000000000003";

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "based-data-live-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn migration(&self, name: &str, up: &str, snap: &str) {
        let dir = self.0.join("migrations").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("up.mig"), up).unwrap();
        std::fs::write(dir.join("schema.snap"), snap).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn exec(backend: &dyn Backend, sql: &str, params: &[SqlValue]) {
    let mut db = backend.checkout("").await.unwrap();
    db.execute(sql, params)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
}

async fn strings(backend: &dyn Backend, sql: &str, col: &str) -> Vec<String> {
    let mut db = backend.checkout("").await.unwrap();
    fetch_all(db.fetch(sql, &[]))
        .await
        .unwrap()
        .iter()
        .map(|r| match &r[col] {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect()
}

/// The whole scenario on one live backend, its tables already dropped.
async fn scenario(backend: &dyn Backend, dialect: Dialect) {
    let q = |s: &str| dialect.quote(s);
    let s = Scratch::new(dialect.name());
    let before = Snapshot::parse(BEFORE).unwrap();
    s.migration(
        "0001_init",
        &render_up(&diff_snapshots(&Snapshot::default(), &before)),
        BEFORE,
    );
    let migs = load_migrations(&s.0, dialect).unwrap();
    apply(backend, dialect, &migs, &ApplyOpts::default())
        .await
        .unwrap();

    for (id, name) in [(C1, "Bolt"), (C2, "Arena Only"), (C3, "Counter")] {
        exec(
            backend,
            &format!(
                "INSERT INTO {} ({}, {}, {}) VALUES ('{id}', '{name}', {})",
                q("mig_card"),
                q("id"),
                q("name"),
                q("paper"),
                dialect.bool_lit(false)
            ),
            &[],
        )
        .await;
    }
    for (n, card, digital, set) in [
        (4, C1, false, "lea"),
        (5, C2, true, "arena"),
        (6, C3, false, "ice"),
    ] {
        exec(
            backend,
            &format!(
                "INSERT INTO {} ({}, {}, {}, {}) VALUES ('00000000-0000-4000-8000-00000000000{n}', '{card}', '{{\"set\":\"{set}\"}}', {})",
                q("mig_card_printing"),
                q("id"),
                q("card_id"),
                q("data"),
                q("digital"),
                dialect.bool_lit(digital)
            ),
            &[],
        )
        .await;
    }
    // An adopted database already holds one reference row; the upsert overwrites it.
    exec(
        backend,
        &format!(
            "INSERT INTO {} ({}, {}, {}) VALUES ('00000000-0000-4000-8000-000000000009', 'commander', 'Old')",
            q("mig_format"),
            q("id"),
            q("key"),
            q("name")
        ),
        &[],
    )
    .await;

    let now = after();
    s.migration("0002_backfill", BACKFILL, &now);
    let migs = load_migrations(&s.0, dialect).unwrap();
    let report = apply(backend, dialect, &migs, &ApplyOpts::default())
        .await
        .unwrap();
    assert_eq!(report.applied, vec!["0002_backfill"]);

    let sets = strings(
        backend,
        &format!(
            "SELECT {} FROM {} ORDER BY {}",
            q("set_code"),
            q("mig_card_printing"),
            q("id")
        ),
        "set_code",
    )
    .await;
    assert_eq!(sets, ["lea", "arena", "ice"]);
    let paper = strings(
        backend,
        &format!(
            "SELECT {} FROM {} WHERE {} = {} ORDER BY {}",
            q("name"),
            q("mig_card"),
            q("paper"),
            dialect.bool_lit(true),
            q("name")
        ),
        "name",
    )
    .await;
    assert_eq!(paper, ["Bolt", "Counter"]);
    let formats = strings(
        backend,
        &format!(
            "SELECT {} FROM {} ORDER BY {}",
            q("name"),
            q("mig_format"),
            q("key")
        ),
        "name",
    )
    .await;
    assert_eq!(formats, ["Brawl", "Commander"]);

    // A chunked step interrupted after the batch ending at C1 resumes past it.
    s.migration(
        "0003_reset",
        "format 2\nupdate all mig_card { paper = false } chunk 1\n",
        &now,
    );
    let migs = load_migrations(&s.0, dialect).unwrap();
    exec(
        backend,
        &format!(
            "INSERT INTO {} ({}, {}, {}, {}, {}) VALUES ('0003_reset', '{}', 0, '[\"''{C1}''\"]', 1)",
            q("_based_migration_progress"),
            q("id"),
            q("content_hash"),
            q("segment"),
            q("cursor"),
            q("rows_done"),
            migs[2].up_hash
        ),
        &[],
    )
    .await;
    apply(backend, dialect, &migs, &ApplyOpts::default())
        .await
        .unwrap();
    let still = strings(
        backend,
        &format!(
            "SELECT {} FROM {} WHERE {} = {} ORDER BY {}",
            q("name"),
            q("mig_card"),
            q("paper"),
            dialect.bool_lit(true),
            q("name")
        ),
        "name",
    )
    .await;
    assert_eq!(
        still,
        ["Bolt"],
        "the resumed run skipped the batch already written"
    );
}

const TABLES: [&str; 5] = [
    "mig_card",
    "mig_card_printing",
    "mig_format",
    "_based_migrations",
    "_based_migration_progress",
];

#[tokio::test]
async fn data_steps_run_against_live_mariadb() {
    let Some(container) = MariaDbContainer::start().await else {
        return;
    };
    let drops: String = TABLES
        .iter()
        .map(|t| format!("DROP TABLE IF EXISTS `{t}`;\n"))
        .collect();
    container.exec_batch(&drops).await;
    let router = ShardRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live MariaDB: {e:?}"));
    scenario(&router, Dialect::MariaDb).await;
    round_trip(&router, Dialect::MariaDb).await;
}

#[tokio::test]
async fn data_steps_run_against_live_postgres() {
    let Some(container) = PostgresContainer::start().await else {
        return;
    };
    let drops: String = TABLES
        .iter()
        .map(|t| format!("DROP TABLE IF EXISTS \"{t}\";\n"))
        .collect();
    container.exec_batch(&drops).await;
    let router = PgRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live Postgres: {e:?}"));
    scenario(&router, Dialect::Postgres).await;
    round_trip(&router, Dialect::Postgres).await;
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

/// Up → down → up through gen's typed down: a rename, then dropping an indexed column (the
/// index comes off first), keeping the row throughout.
async fn round_trip(backend: &dyn Backend, dialect: Dialect) {
    let q = |s: &str| dialect.quote(s);
    for t in [
        "round_widget",
        "_based_migrations",
        "_based_migration_progress",
    ] {
        exec(backend, &format!("DROP TABLE IF EXISTS {}", q(t)), &[]).await;
    }
    let s = Scratch::new(&format!("round-{}", dialect.name()));
    round_trip_tree(&s.0);
    let migs = load_migrations(&s.0, dialect).unwrap();
    // 0003 drops a column, so the forward runs vouch for it.
    let up = ApplyOpts {
        allow_destructive: true,
        ..ApplyOpts::default()
    };
    apply(backend, dialect, &migs, &up).await.unwrap();
    exec(
        backend,
        &format!(
            "INSERT INTO {} ({}, {}) VALUES ('{C1}', 'Gizmo')",
            q("round_widget"),
            q("id"),
            q("name")
        ),
        &[],
    )
    .await;
    let down = ApplyOpts {
        direction: based_runtime::migrate::Direction::Down,
        ..ApplyOpts::default()
    };
    let report = apply(backend, dialect, &migs, &down).await.unwrap();
    assert_eq!(report.rolled_back, vec!["0003_drop_size"]);
    let sizes = strings(
        backend,
        &format!("SELECT {} FROM {}", q("size"), q("round_widget")),
        "size",
    )
    .await;
    assert_eq!(sizes, ["null"], "the dropped column is back, empty");
    let report = apply(backend, dialect, &migs, &down).await.unwrap();
    assert_eq!(report.rolled_back, vec!["0002_rename"]);
    let titles = strings(
        backend,
        &format!("SELECT {} FROM {}", q("title"), q("round_widget")),
        "title",
    )
    .await;
    assert_eq!(titles, ["Gizmo"]);
    let report = apply(backend, dialect, &migs, &up).await.unwrap();
    assert_eq!(report.applied, vec!["0002_rename", "0003_drop_size"]);
    let names = strings(
        backend,
        &format!("SELECT {} FROM {}", q("name"), q("round_widget")),
        "name",
    )
    .await;
    assert_eq!(names, ["Gizmo"]);
}
