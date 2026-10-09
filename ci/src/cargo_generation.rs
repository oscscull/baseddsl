use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use std::{fs, path::Path};

pub fn verify() -> Result<()> {
    let root = command::root();
    let tree = command::run(
        command::cargo(),
        &[
            "tree",
            "-p",
            "based-build",
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        &root,
        &Environment::new(),
    )?;
    for package in [
        "based-runtime",
        "sqlx",
        "sqlx-core",
        "tokio",
        "axum",
        "hyper",
        "reqwest",
    ] {
        ensure!(
            !tree
                .lines()
                .any(|line| line.split_whitespace().next() == Some(package)),
            "build helper depends on {package}"
        );
    }
    let scratch = tempfile::tempdir()?;
    let app = scratch.path().join("app");
    crate::files::copy_tree(&root.join("ci/fixtures/cargo-generation"), &app)?;
    crate::files::replace(
        &app.join("Cargo.toml"),
        "../../../crates/",
        &format!("{}/crates/", root.display()),
    )?;
    let target = scratch.path().join("target");
    let env = Environment::from([("CARGO_TARGET_DIR".into(), target.display().to_string())]);
    let blocked = scratch.path().join("blocked-tools");
    fs::create_dir(&blocked)?;
    for tool in ["based", "rustfmt"] {
        crate::files::executable(
            &blocked.join(tool),
            "#!/bin/sh\necho unexpected external generation tool >&2\nexit 97\n",
        )?;
    }
    let mut build_env = env.clone();
    let path = std::env::join_paths(std::iter::once(blocked).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))?;
    build_env.insert("PATH".into(), path.to_string_lossy().into());
    command::run(command::cargo(), &["run"], &app, &build_env)?;
    let clients = crate::artifacts::find(&target, "cargo-generation-consumer", "client.rs")?;
    ensure!(clients.len() == 1, "expected exactly one generated client");
    let client = &clients[0];
    let record = client.with_file_name("verification.txt");
    ensure!(
        fs::read_to_string(&record)? == "checked=true changed=true\n",
        "initial generation missing"
    );
    unchanged(&app, &build_env, client, &record)?;
    fs::remove_file(client)?;
    build(&app, &build_env)?;
    ensure!(
        fs::read_to_string(&record)? == "checked=true changed=true\n",
        "missing output not rebuilt"
    );
    let main = app.join("src/main.rs");
    fs::write(
        &main,
        fs::read_to_string(&main)? + "\nfn unrelated_rust_edit() {}\n",
    )?;
    unchanged(&app, &build_env, client, &record)?;
    let before = fs::read(client)?;
    command::run(command::cargo(), &["fmt", "--all"], &app, &env)?;
    ensure!(
        fs::read(client)? == before,
        "formatter changed generated client"
    );
    schema(&app, &build_env, client, &record)?;
    crate::files::replace(
        &app.join("based.toml"),
        "client_mode = \"embedded\"",
        "client_mode = \"wire\"",
    )?;
    command::fail(command::cargo(), &["build", "--locked"], &app, &build_env)?;
    ensure!(
        !fs::read_to_string(client)?.contains("pub fn embedded("),
        "mode change ignored"
    );
    crate::files::replace(
        &app.join("based.toml"),
        "client_mode = \"wire\"",
        "client_mode = \"embedded\"",
    )?;
    build(&app, &build_env)?;
    crate::files::replace(&app.join("based.toml"), "root = \"schema\"\n", "")?;
    build(&app, &build_env)?;
    fs::write(
        &main,
        fs::read_to_string(&main)? + "\nfn shared_root_rust_edit() {}\n",
    )?;
    unchanged(&app, &build_env, client, &record)?;
    ensure!(
        !walkdir::WalkDir::new(&app)
            .into_iter()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == "client.rs"),
        "client escaped OUT_DIR"
    );
    Ok(())
}

fn build(app: &Path, env: &Environment) -> Result<()> {
    command::run(command::cargo(), &["build", "--locked"], app, env)?;
    Ok(())
}

fn unchanged(app: &Path, env: &Environment, client: &Path, record: &Path) -> Result<()> {
    let before = fs::metadata(client)?.modified()?;
    let checked = fs::metadata(record)?.modified()?;
    build(app, env)?;
    ensure!(
        fs::metadata(client)?.modified()? == before,
        "unchanged client rewritten"
    );
    if fs::metadata(record)?.modified()? != checked {
        ensure!(
            fs::read_to_string(record)? == "checked=false changed=false\n",
            "compiler ran for unrelated input"
        );
    }
    Ok(())
}

fn schema(app: &Path, env: &Environment, client: &Path, record: &Path) -> Result<()> {
    let source = app.join("schema/item.bsl");
    let schema = fs::read_to_string(&source)?;
    let original = fs::read(client)?;
    let timestamp = fs::metadata(client)?.modified()?;
    fs::write(&source, schema.clone() + "\n# Non-semantic edit\n")?;
    build(app, env)?;
    ensure!(
        fs::read_to_string(record)? == "checked=true changed=false\n",
        "schema edit not checked"
    );
    ensure!(
        fs::metadata(client)?.modified()? == timestamp,
        "identical output rewritten"
    );
    let extra = app.join("schema/nested/extra.bsl");
    fs::create_dir(extra.parent().unwrap())?;
    fs::write(&extra, "query other_items() -> ItemRow[] { list Item; }\n")?;
    build(app, env)?;
    ensure!(
        fs::read_to_string(client)?.contains("pub async fn other_items"),
        "new input not generated"
    );
    fs::remove_file(&extra)?;
    fs::remove_dir(extra.parent().unwrap())?;
    build(app, env)?;
    ensure!(fs::read(client)? == original, "deleted input retained");
    fs::write(&source, "Item { id: serial, name:\n")?;
    let error = command::fail(command::cargo(), &["build", "--locked"], app, env)?;
    ensure!(
        error.contains("item.bsl:") && error.contains("check failed:"),
        "missing source diagnostic: {error}"
    );
    ensure!(
        fs::read(client)? == original,
        "failed generation replaced client"
    );
    fs::write(source, schema)?;
    build(app, env)?;
    Ok(())
}
