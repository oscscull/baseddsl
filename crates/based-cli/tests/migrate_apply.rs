//! `based migrate apply` end-to-end against a real SQLite file: the destructive-gate
//! contract (#18). A destructive mid-chain migration must **stop with a non-zero exit** (a
//! refused step is a failure to reach the target schema, never a success a script can walk
//! past), apply only the safe migrations before it, and report the partial state it left —
//! which migrations landed, and that the database is partially migrated. Runs the compiled
//! binary; SQLite is always linked, so no DB daemon is needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway project dir under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let mut dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("based-apply-cli-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, rel: &str, contents: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    fn migration(&self, name: &str, up: &str, snap: &str) {
        self.write(&format!("migrations/{name}/up.mig"), up);
        self.write(&format!("migrations/{name}/schema.snap"), snap);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A project whose migration chain has a destructive step (0003 drops a column) with a safe
/// migration on either side, so a plain apply stops mid-chain.
fn project_with_destructive_midchain(tag: &str) -> Scratch {
    let s = Scratch::new(tag);
    s.write("based.toml", "dialect = \"sqlite\"\n");
    s.write("model.bsl", "Widget { id: Id, size: int? }\n");
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
    s.migration(
        "0003_drop_name",
        "drop column widget.name  # DESTRUCTIVE\n",
        "snapshot v1 dialect=neutral\n\ntable widget\n  column size int null\n",
    );
    s.migration(
        "0004_add_color",
        "add column widget.color text null\n",
        "snapshot v1 dialect=neutral\n\ntable widget\n  column size int null\n  column color text null\n",
    );
    s
}

fn run_apply(root: &Path, db: &Path, allow_destructive: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_based"));
    cmd.arg("migrate")
        .arg("apply")
        .arg(root)
        .arg("--database-url")
        .arg(db);
    if allow_destructive {
        cmd.arg("--allow-destructive");
    }
    cmd.output().expect("run based migrate apply")
}

#[test]
fn destructive_gate_exits_nonzero_and_reports_partial_state() {
    let s = project_with_destructive_midchain("gate");
    let db = s.0.join("fresh.db");

    let out = run_apply(&s.0, &db, false);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // A refused destructive step is a failure, not a success: exit 2 (the usage class), so a
    // scripted `apply && next` halts instead of continuing on a half-migrated database.
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 at the destructive gate\nstdout: {stdout}\nstderr: {stderr}"
    );
    // The safe migrations that did land are named, so the partial state is visible.
    assert!(stdout.contains("applied 0001_init"), "stdout: {stdout}");
    assert!(stdout.contains("applied 0002_add_size"), "stdout: {stdout}");
    // The gate names the offending migration and says the database is partially migrated.
    assert!(stderr.contains("0003_drop_name"), "stderr: {stderr}");
    assert!(stderr.contains("partially migrated"), "stderr: {stderr}");
    assert!(
        stderr.contains("--allow-destructive"),
        "stderr: {stderr}"
    );
    // 0004 (after the gate) must not have applied.
    assert!(!stdout.contains("applied 0004_add_color"), "stdout: {stdout}");
}

#[test]
fn re_running_with_the_ack_completes_the_chain() {
    let s = project_with_destructive_midchain("ack");
    let db = s.0.join("fresh.db");

    // Plain apply stops at the gate (non-zero).
    assert_eq!(run_apply(&s.0, &db, false).status.code(), Some(2));

    // Re-run with the ack: the partial state rolls forward to completion, exit 0.
    let out = run_apply(&s.0, &db, true);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "expected success\nstdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("applied 0003_drop_name"), "stdout: {stdout}");
    assert!(stdout.contains("applied 0004_add_color"), "stdout: {stdout}");
}
