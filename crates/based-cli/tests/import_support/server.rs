//! Execute the same fresh consumer proof as SQLite; credentials travel only through environment.
use std::{path::Path, process::Command};

pub fn verify(dialect: &str) {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().canonicalize().unwrap();
    std::fs::create_dir(app.join("models")).unwrap();
    std::fs::write(
        app.join("based.toml"),
        format!("dialect = '{dialect}'\nroot = 'models'\n[generate]\nclient = 'src/based_client.rs'\nclient_mode = 'embedded'\n"),
    ).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = Command::new("python3")
        .arg(repo.join("ci/check-import-server-consumer.py"))
        .args(["--based", env!("CARGO_BIN_EXE_based"), "--project"])
        .arg(&app)
        .args(["--dialect", dialect])
        .output()
        .unwrap();
    // The Python helper catches driver errors; never print raw SQLx connection payloads here.
    assert!(
        output.status.success(),
        "fresh {dialect} consumer failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
}
