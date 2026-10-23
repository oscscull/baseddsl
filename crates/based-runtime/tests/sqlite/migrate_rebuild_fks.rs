//! A SQLite table rebuild of a parent with `on_delete: cascade` children, applied on a
//! connection that enforces foreign keys (both drivers turn them on). The children must keep
//! referencing the real parent table and keep their rows, and enforcement must be back on
//! afterwards — including when the migration fails its foreign key check.

use std::path::PathBuf;

use based_codegen::migrate::{
    diff_snapshots, render_up, ColumnSnap, ForeignKeySnap, Snapshot, TableSnap,
};
use based_runtime::fetch_all;
use based_runtime::migrate::{apply, load_migrations, ApplyOpts, Dialect, MigrateError};
use based_runtime::run::{Backend, Db};

use crate::driver::SqliteBackend;

/// A throwaway project dir under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "based-rebuild-fks-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn migration(&self, name: &str, prev: &Snapshot, next: &Snapshot) {
        let dir = self.0.join("migrations").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("up.mig"), render_up(&diff_snapshots(prev, next))).unwrap();
        std::fs::write(dir.join("schema.snap"), next.render()).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn column(name: &str, ty: &str, nullable: bool, default: Option<&str>) -> ColumnSnap {
    ColumnSnap {
        name: name.to_string(),
        ty: ty.to_string(),
        nullable,
        default: default.map(str::to_string),
        unique: false,
        fk: None,
        generated: None,
    }
}

fn table(name: &str, columns: Vec<ColumnSnap>) -> TableSnap {
    TableSnap {
        name: name.to_string(),
        schema: None,
        soft_delete: None,
        created: None,
        updated: None,
        scope_alts: Vec::new(),
        sort: Vec::new(),
        no_id: false,
        pk: Vec::new(),
        columns,
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
    }
}

/// A child of `item`, deleted with it.
fn child(name: &str) -> TableSnap {
    let mut t = table(name, vec![column("item_id", "uuid", false, None)]);
    t.foreign_keys.push(ForeignKeySnap {
        columns: vec!["item_id".to_string()],
        ref_table: "item".to_string(),
        ref_schema: None,
        ref_columns: vec!["id".to_string()],
        on_delete: Some("cascade".to_string()),
        on_update: None,
    });
    t
}

/// `item` with its `turn` column either `not null default 0` or nullable, plus two children.
fn schema(turn_nullable: bool) -> Snapshot {
    let turn = if turn_nullable {
        column("turn", "int", true, None)
    } else {
        column("turn", "int", false, Some("0"))
    };
    Snapshot {
        scopes: Vec::new(),
        tables: vec![table("item", vec![turn]), child("ask"), child("watch")],
        renames: Vec::new(),
    }
}

/// A database at v1 with one item and a child row in each child table, and the v2 rebuild
/// migration (`item.turn` becomes nullable) written but not applied.
async fn seeded(tag: &str) -> (Scratch, SqliteBackend) {
    let s = Scratch::new(tag);
    let v1 = schema(false);
    s.migration("0001_init", &Snapshot::default(), &v1);
    let backend = SqliteBackend::in_memory().unwrap();
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    apply(&backend, Dialect::Sqlite, &migs, &ApplyOpts::default())
        .await
        .unwrap();
    let mut db = backend.checkout("").await.unwrap();
    run(&mut *db, "INSERT INTO `item` (`id`, `turn`) VALUES ('i1', 1)").await;
    run(&mut *db, "INSERT INTO `ask` (`id`, `item_id`) VALUES ('a1', 'i1')").await;
    run(&mut *db, "INSERT INTO `watch` (`id`, `item_id`) VALUES ('w1', 'i1')").await;
    s.migration("0002_turn_optional", &v1, &schema(true));
    (s, backend)
}

async fn run(db: &mut dyn Db, sql: &str) {
    db.execute(sql, &[]).await.unwrap();
}

async fn scalar(db: &mut dyn Db, sql: &str) -> serde_json::Value {
    let rows = fetch_all(db.fetch(sql, &[])).await.unwrap();
    rows[0].values().next().cloned().unwrap()
}

async fn apply_all(s: &Scratch, backend: &SqliteBackend) -> Result<(), MigrateError> {
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    apply(backend, Dialect::Sqlite, &migs, &ApplyOpts::default())
        .await
        .map(|_| ())
}

#[tokio::test]
async fn a_parent_rebuild_keeps_its_cascade_children_intact() {
    let (s, backend) = seeded("keeps").await;
    apply_all(&s, &backend).await.expect("rebuild applies");

    let mut db = backend.checkout("").await.unwrap();
    for child in ["ask", "watch"] {
        let target = scalar(
            &mut *db,
            &format!("SELECT \"table\" FROM pragma_foreign_key_list('{child}')"),
        )
        .await;
        assert_eq!(target.as_str(), Some("item"), "{child} references item");
        let count = scalar(&mut *db, &format!("SELECT COUNT(*) FROM `{child}`")).await;
        assert_eq!(count.as_i64(), Some(1), "{child} kept its row");
    }
    let leftovers = scalar(
        &mut *db,
        "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%__based_rebuild%' \
         OR sql LIKE '%__based_rebuild%'",
    )
    .await;
    assert_eq!(leftovers.as_i64(), Some(0), "nothing names the rebuild copy");
    let nullable = scalar(
        &mut *db,
        "SELECT `notnull` FROM pragma_table_info('item') WHERE name = 'turn'",
    )
    .await;
    assert_eq!(nullable.as_i64(), Some(0), "turn is nullable");

    let fks = scalar(&mut *db, "PRAGMA foreign_keys").await;
    assert_eq!(fks.as_i64(), Some(1), "foreign keys are enforced again");
    run(&mut *db, "UPDATE `item` SET `turn` = NULL").await;
    run(&mut *db, "DELETE FROM `item` WHERE `id` = 'i1'").await;
    for child in ["ask", "watch"] {
        let count = scalar(&mut *db, &format!("SELECT COUNT(*) FROM `{child}`")).await;
        assert_eq!(count.as_i64(), Some(0), "deleting the item cascades into {child}");
    }
}

#[tokio::test]
async fn a_rebuild_that_breaks_a_reference_rolls_back_and_restores_enforcement() {
    let (s, backend) = seeded("violation").await;
    {
        // An orphan child, written with enforcement briefly off on the one connection.
        let mut db = backend.checkout("").await.unwrap();
        run(&mut *db, "PRAGMA foreign_keys = OFF").await;
        run(&mut *db, "INSERT INTO `ask` (`id`, `item_id`) VALUES ('a2', 'gone')").await;
        run(&mut *db, "PRAGMA foreign_keys = ON").await;
    }

    let err = apply_all(&s, &backend).await.expect_err("check fails");
    assert!(err.to_string().contains("foreign key check failed"), "{err}");

    let mut db = backend.checkout("").await.unwrap();
    let fks = scalar(&mut *db, "PRAGMA foreign_keys").await;
    assert_eq!(fks.as_i64(), Some(1), "foreign keys are enforced again");
    let not_null = scalar(
        &mut *db,
        "SELECT `notnull` FROM pragma_table_info('item') WHERE name = 'turn'",
    )
    .await;
    assert_eq!(not_null.as_i64(), Some(1), "the rebuild rolled back");
    let applied = scalar(&mut *db, "SELECT COUNT(*) FROM _based_migrations").await;
    assert_eq!(applied.as_i64(), Some(1), "only 0001 is recorded");
    let asks = scalar(&mut *db, "SELECT COUNT(*) FROM `ask`").await;
    assert_eq!(asks.as_i64(), Some(2), "no child rows were lost");
}
