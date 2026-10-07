//! Real CLI failure, persisted effects and manual recovery for each backend.
#[path = "support/project.rs"]
mod project;

use based_codegen::Dialect;
use based_runtime::{fetch_all, Backend, SqliteBackend};
use project::{success, Project};

fn fixture(dialect: &str) -> Project {
    let p = Project::new();
    p.write("based.toml", &format!("dialect = \"{dialect}\"\n"));
    p.write(
        "migrations/0001_init/schema.snap",
        "snapshot v1 dialect=neutral\n\ntable widget\n  column name text not_null\n",
    );
    p.write(
        "migrations/0001_init/up.mig",
        "create table widget {\n  column name text not_null\n}\n",
    );
    p
}

fn add_failure(p: &Project, dialect: &str) {
    p.write("migrations/0002_add_size/schema.snap", "snapshot v1 dialect=neutral\n\ntable widget\n  column name text not_null\n  column size int null\n");
    p.write("migrations/0002_add_size/up.mig", &format!("add column widget.size int null\nraw({dialect}) `ALTER TABLE widget ADD COLUMN name TEXT`\n"));
}

async fn verify_failure(backend: &dyn Backend, dialect: Dialect, url: &str) {
    let label = match dialect {
        Dialect::MariaDb => "mariadb",
        Dialect::Postgres => "postgres",
        Dialect::Sqlite => "sqlite",
        Dialect::MySql => unreachable!(),
    };
    let p = fixture(label);
    {
        let mut db = backend.checkout("").await.unwrap();
        db.execute("DROP TABLE IF EXISTS widget", &[])
            .await
            .unwrap();
        db.execute("DROP TABLE IF EXISTS _based_migrations", &[])
            .await
            .unwrap();
    }
    success(p.run("", &["migrate", "apply", "--database-url", url]));
    {
        let mut db = backend.checkout("").await.unwrap();
        db.execute("INSERT INTO widget (id, name) VALUES ('00000000-0000-4000-8000-000000000001', 'keep me')", &[])
            .await
            .unwrap();
    }
    add_failure(&p, label);
    let output = p.run("", &["migrate", "apply", "--database-url", url]);
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("0002_add_size"), "{diagnostic}");
    assert!(diagnostic.contains("statement 2 of 2"), "{diagnostic}");
    assert!(diagnostic.contains("before retry"), "{diagnostic}");
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("SELECT id FROM _based_migrations ORDER BY id", &[]))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "0001_init");
    let column_sql = match dialect {
        Dialect::MariaDb => "SELECT column_name AS name FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = 'widget'",
        Dialect::Postgres => "SELECT column_name AS name FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'widget'",
        _ => "SELECT name FROM pragma_table_info('widget')",
    };
    let columns = fetch_all(db.fetch(column_sql, &[])).await.unwrap();
    let persisted = columns.iter().any(|r| r["name"] == "size");
    assert_eq!(persisted, dialect == Dialect::MariaDb);
    if persisted {
        assert!(diagnostic.contains("DDL may already be committed"));
        // Runbook: only remove the new, unused column, preserving the seeded name.
        db.execute("ALTER TABLE widget DROP COLUMN size", &[])
            .await
            .unwrap();
    }
    drop(db);
    // This migration has never completed, so correcting its raw step does not edit history.
    p.write(
        "migrations/0002_add_size/up.mig",
        "add column widget.size int null\n",
    );
    success(p.run("", &["migrate", "apply", "--database-url", url]));
    success(p.run("", &["migrate", "apply", "--database-url", url]));
    let mut db = backend.checkout("").await.unwrap();
    let rows = fetch_all(db.fetch("SELECT name, size FROM widget", &[]))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "keep me");
    assert!(rows[0]["size"].is_null());
    let ledger = fetch_all(db.fetch("SELECT id FROM _based_migrations ORDER BY id", &[]))
        .await
        .unwrap();
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[1]["id"], "0002_add_size");
}

#[tokio::test]
async fn sqlite_failed_ddl_rolls_back_and_recovers() {
    let p = Project::new();
    let file = p.0.join("failure.db");
    let url = file.to_str().unwrap();
    let backend = SqliteBackend::open(url).unwrap();
    verify_failure(&backend, Dialect::Sqlite, url).await;
}

#[cfg(feature = "mariadb")]
#[tokio::test]
async fn mariadb_failed_ddl_persists_and_recovers() {
    let Ok(url) = std::env::var("TEST_MARIADB_URL") else {
        return;
    };
    let backend = based_runtime::driver::ShardRouter::single(&url, Default::default()).unwrap();
    verify_failure(&backend, Dialect::MariaDb, &url).await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_failed_ddl_rolls_back_and_recovers() {
    let Ok(url) = std::env::var("TEST_POSTGRES_URL") else {
        return;
    };
    let backend = based_runtime::PgRouter::single(&url, Default::default()).unwrap();
    verify_failure(&backend, Dialect::Postgres, &url).await;
}
