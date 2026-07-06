//! `based migrate gen` end-to-end: scaffold a tiny project in a temp dir, run the
//! compiled binary, and assert the migration files it writes (E2). Offline — no DB.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway project dir under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let mut dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!(
            "based-migrate-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn write(&self, rel: &str, contents: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_gen(root: &Path, name: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_based"));
    cmd.arg("migrate").arg("gen").arg(root);
    if let Some(n) = name {
        cmd.arg(n);
    }
    cmd.output().expect("run based migrate gen")
}

const MANIFEST: &str = "dialect = \"mariadb\"\nclient = \"rust\"\n";

#[test]
fn gen_writes_init_then_is_a_no_op_when_unchanged() {
    let s = Scratch::new("init");
    s.write("based.toml", MANIFEST);
    s.write(
        "shop.bsl",
        "Org { name: text }\nProduct { org: Org  name: text }\n",
    );

    // First run: writes 0001_init with both artifacts.
    let out = run_gen(&s.0, Some("init"));
    assert!(out.status.success(), "gen failed: {out:#?}");
    let init = s.0.join("migrations/0001_init");
    assert!(init.join("up.mig").is_file(), "up.mig not written");
    assert!(
        init.join("schema.snap").is_file(),
        "schema.snap not written"
    );

    let up = std::fs::read_to_string(init.join("up.mig")).unwrap();
    assert!(up.contains("create table org {"), "\n{up}");
    assert!(up.contains("create table product {"), "\n{up}");

    // Second run with no schema change: writes nothing, exits clean, says so.
    let out2 = run_gen(&s.0, None);
    assert!(out2.status.success());
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout.contains("no schema changes"), "stdout: {stdout}");
    assert!(
        !s.0.join("migrations/0002_schema_update").exists(),
        "a no-op must not write a second migration"
    );
}

#[test]
fn gen_writes_the_next_incremental_migration() {
    let s = Scratch::new("incr");
    s.write("based.toml", MANIFEST);
    s.write("shop.bsl", "Product { name: text }\n");

    let out = run_gen(&s.0, Some("init"));
    assert!(out.status.success(), "{out:#?}");

    // Evolve the schema: a nullable column + an index on it.
    s.write(
        "shop.bsl",
        "Product { name: text  barcode: text?  @index barcode }\n",
    );
    let out2 = run_gen(&s.0, Some("add barcode"));
    assert!(out2.status.success(), "{out2:#?}");

    // Slug is snake-cased from the name; number is the next in sequence.
    let next = s.0.join("migrations/0002_add_barcode");
    assert!(next.join("up.mig").is_file(), "0002 up.mig not written");
    let up = std::fs::read_to_string(next.join("up.mig")).unwrap();
    assert!(
        up.contains("add column product.barcode text null"),
        "\n{up}"
    );
    assert!(
        up.contains("add index idx_product_barcode (barcode)"),
        "\n{up}"
    );

    // The new schema.snap is the fresh baseline (includes the added column).
    let snap = std::fs::read_to_string(next.join("schema.snap")).unwrap();
    assert!(snap.contains("column barcode text null"), "\n{snap}");
}
