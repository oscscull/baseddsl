//! `based migrate apply` against a **real** MariaDB server, over Docker. The live twin of
//! `migrate_apply.rs`: it writes a real `migrations/` tree, loads it
//! for the MariaDB dialect, and applies it through a live `ShardRouter` (the concrete MariaDB
//! `Backend`) — so a passing run proves the apply engine + `_based_migrations` ledger work
//! against a genuine server (DDL, the ledger insert, the tamper guard, a re-apply no-op), not just
//! compile-verified. When the Docker daemon is unreachable the harness returns `None` and each test
//! **skips cleanly**, so `cargo test --workspace --all-features` stays green with no daemon.

#![cfg(feature = "docker-tests")]

#[path = "support/docker_mariadb.rs"]
mod docker_mariadb;

use std::path::PathBuf;

use based_codegen::Dialect;
use based_runtime::driver::{PoolConfig, ShardRouter};
use based_runtime::fetch_all;
use based_runtime::migrate::{apply, load_migrations, ApplyOpts, MigrateError};
use based_runtime::run::DbRead;

use docker_mariadb::MariaDbContainer;

/// A throwaway migrations dir under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let mut dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("based-apply-maria-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn migration(&self, name: &str, up: &str, snap: &str) {
        let dir = self.0.join("migrations").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("up.mig"), up).unwrap();
        std::fs::write(dir.join("schema.snap"), snap).unwrap();
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

/// The 0001-create-widget + 0002-add-size migration tree the tests apply.
fn scenario() -> Scratch {
    let s = Scratch::new();
    s.migration(
        "0001_init",
        "create table widget {\n  column name text not_null\n}\n",
        "snapshot v1 dialect=neutral\n\ntable widget\n  column name text not_null\n",
    );
    s.migration(
        "0002_add_size",
        "add column widget.size int null\n",
        "snapshot v1 dialect=neutral\n\ntable widget\n  column name text not_null\n  column size int null\n",
    );
    s
}

/// Bring up a live MariaDB; `None` (skip) when Docker is unavailable. Drops this scenario's
/// table + the migrations ledger first, so a run against a *persistent* external server
/// (`TEST_MARIADB_URL`) starts clean and is re-runnable (a no-op on a fresh container).
async fn live() -> Option<(ShardRouter, MariaDbContainer)> {
    let container = MariaDbContainer::start().await?;
    let router = ShardRouter::single(&container.url(), PoolConfig::default())
        .unwrap_or_else(|e| panic!("connect to live MariaDB: {e:?}"));
    container
        .exec_batch("DROP TABLE IF EXISTS `widget`;\nDROP TABLE IF EXISTS `_based_migrations`;")
        .await;
    Some((router, container))
}

async fn ledger_count(router: &ShardRouter) -> i64 {
    let mut db = router.checkout("").await.unwrap();
    fetch_all(db.fetch("SELECT COUNT(*) AS c FROM `_based_migrations`", &[]))
        .await
        .unwrap()[0]["c"]
        .as_i64()
        .unwrap()
}

async fn widget_has_size(router: &ShardRouter) -> bool {
    let mut db = router.checkout("").await.unwrap();
    let n = fetch_all(db.fetch(
        "SELECT COUNT(*) AS c FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = 'widget' AND column_name = 'size'",
        &[],
    ))
    .await
    .unwrap();
    n[0]["c"].as_i64().unwrap() == 1
}

#[tokio::test]
async fn apply_runs_migrations_against_live_mariadb() {
    let Some((router, _guard)) = live().await else {
        return;
    };
    let s = scenario();
    let migs = load_migrations(&s.0, Dialect::MariaDb).unwrap();

    // Fresh apply: both migrations run their real MariaDB DDL, both ledger rows land.
    let report = apply(&router, Dialect::MariaDb, &migs, &ApplyOpts::default())
        .await
        .unwrap();
    assert_eq!(report.applied, vec!["0001_init", "0002_add_size"]);
    assert!(
        widget_has_size(&router).await,
        "0002 added the `size` column live"
    );
    assert_eq!(ledger_count(&router).await, 2);

    // Re-apply: nothing pending, the ledger is unchanged (idempotent).
    let report = apply(&router, Dialect::MariaDb, &migs, &ApplyOpts::default())
        .await
        .unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(ledger_count(&router).await, 2);
}

#[tokio::test]
async fn editing_an_applied_migration_is_a_tamper_error_live() {
    let Some((router, _guard)) = live().await else {
        return;
    };
    let s = scenario();
    let migs = load_migrations(&s.0, Dialect::MariaDb).unwrap();
    apply(&router, Dialect::MariaDb, &migs, &ApplyOpts::default())
        .await
        .unwrap();

    // Edit an applied migration's up.mig; the recorded ledger hash no longer matches.
    std::fs::write(
        s.up_path("0002_add_size"),
        "add column widget.size int not_null\n",
    )
    .unwrap();
    let tampered = load_migrations(&s.0, Dialect::MariaDb).unwrap();
    let err = apply(&router, Dialect::MariaDb, &tampered, &ApplyOpts::default())
        .await
        .unwrap_err();
    assert!(matches!(err, MigrateError::Tamper { .. }), "{err}");
}
