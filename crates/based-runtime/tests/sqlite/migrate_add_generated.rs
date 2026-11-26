//! Adding a stored generated column to a populated SQLite table. SQLite's `ADD COLUMN` takes
//! only virtual generated columns, so the migration rebuilds the table: existing rows get the
//! computed value, and a later rebuild leaves the generated column to compute itself.

use std::path::PathBuf;

use based_codegen::migrate::{diff_snapshots, render_up, ColumnSnap, Snapshot, TableSnap};
use based_runtime::fetch_all;
use based_runtime::migrate::{apply, load_migrations, ApplyOpts, Dialect};
use based_runtime::run::{Backend, Db};

use crate::driver::SqliteBackend;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "based-add-generated-{tag}-{}-{nanos}",
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

fn column(name: &str, ty: &str, nullable: bool, generated: Option<&str>) -> ColumnSnap {
    ColumnSnap {
        name: name.to_string(),
        ty: ty.to_string(),
        nullable,
        default: None,
        unique: false,
        fk: None,
        generated: generated.map(str::to_string),
    }
}

fn item(columns: Vec<ColumnSnap>) -> Snapshot {
    Snapshot {
        scopes: Vec::new(),
        tables: vec![TableSnap {
            name: "item".to_string(),
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
        }],
        renames: Vec::new(),
    }
}

async fn run(db: &mut dyn Db, sql: &str) {
    db.execute(sql, &[]).await.unwrap();
}

async fn scalar(db: &mut dyn Db, sql: &str) -> serde_json::Value {
    let rows = fetch_all(db.fetch(sql, &[])).await.unwrap();
    rows[0].values().next().cloned().unwrap()
}

async fn apply_all(s: &Scratch, backend: &SqliteBackend) {
    let migs = load_migrations(&s.0, Dialect::Sqlite).unwrap();
    apply(backend, Dialect::Sqlite, &migs, &ApplyOpts::default())
        .await
        .expect("migrations apply");
}

#[tokio::test]
async fn a_stored_generated_column_is_added_to_existing_rows() {
    let s = Scratch::new("add");
    let price = column("price", "int", false, None);
    let discount = column("discount", "int", false, None);
    let net = column("net", "int", false, Some("price - discount"));
    let v1 = item(vec![price.clone(), discount.clone()]);
    let v2 = item(vec![price.clone(), discount, net.clone()]);
    let v3 = item(vec![
        price,
        column("discount", "int", true, None),
        net,
    ]);

    s.migration("0001_init", &Snapshot::default(), &v1);
    let backend = SqliteBackend::in_memory().unwrap();
    apply_all(&s, &backend).await;
    {
        let mut db = backend.checkout("").await.unwrap();
        run(&mut *db, "INSERT INTO `item` (`id`, `price`, `discount`) VALUES ('i1', 10, 3)").await;
    }

    s.migration("0002_net", &v1, &v2);
    apply_all(&s, &backend).await;
    let mut db = backend.checkout("").await.unwrap();
    let net = scalar(&mut *db, "SELECT `net` FROM `item` WHERE `id` = 'i1'").await;
    assert_eq!(net.as_i64(), Some(7), "the existing row computes net");
    drop(db);

    // A later rebuild of the same table copies the stored columns and recomputes `net`.
    s.migration("0003_discount_optional", &v2, &v3);
    apply_all(&s, &backend).await;
    let mut db = backend.checkout("").await.unwrap();
    run(&mut *db, "UPDATE `item` SET `discount` = 4").await;
    let net = scalar(&mut *db, "SELECT `net` FROM `item` WHERE `id` = 'i1'").await;
    assert_eq!(net.as_i64(), Some(6), "net follows its inputs after the rebuild");
}
