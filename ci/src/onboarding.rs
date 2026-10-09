use crate::{
    command::{self, Environment},
    initializer,
};
use anyhow::{ensure, Result};
use sqlx::Row;
use std::{fs, path::Path};

pub async fn verify(based: &Path) -> Result<()> {
    let based = based.canonicalize()?;
    let scratch = tempfile::tempdir()?;
    let env = crate::source::mirror(scratch.path())?;
    let app = scratch.path().join("app");
    initializer::fresh(&based, &app, "embedded", "sqlite", &env)?;
    let manifest = fs::read_to_string(app.join("Cargo.toml"))?;
    ensure!(
        manifest.contains("git = \"https://github.com/oscscull/baseddsl.git\"")
            && !manifest.contains("path ="),
        "consumer did not use pinned public Git source"
    );
    command::run(
        &based,
        &["migrate", "apply", "--database-url", "local.db"],
        &app,
        &env,
    )?;
    initializer::identity(&command::run(command::cargo(), &["run"], &app, &env)?)?;
    ensure!(
        fs::read_to_string(app.join("Cargo.lock"))?
            .contains(&format!("#{}", command::git(&["rev-parse", "HEAD"])?)),
        "consumer resolved another commit"
    );
    let before = rows(&app, "name").await?;
    ensure!(before.len() == 2, "wrong lesson rows");
    rename(&app)?;
    let error = command::fail(&based, &["gen", "all", "--check"], &app, &env)?;
    ensure!(
        error.to_lowercase().contains("stale"),
        "stale artifacts accepted"
    );
    for args in [
        &["check"][..],
        &["gen", "all"],
        &["migrate", "gen", ".", "rename_item_title"],
        &["migrate", "verify"],
        &["migrate", "render", ".", "--number", "2"],
    ] {
        command::run(&based, args, &app, &env)?;
    }
    ensure!(
        rows(&app, "name").await? == before,
        "generation changed data"
    );
    command::run(
        &based,
        &["migrate", "apply", "--database-url", "local.db"],
        &app,
        &env,
    )?;
    ensure!(
        rows(&app, "title").await? == before,
        "rename changed rows or relationship IDs"
    );
    initializer::identity(&command::run(command::cargo(), &["run"], &app, &env)?)?;
    command::run(command::cargo(), &["fmt", "--check"], &app, &env)?;
    command::run(
        command::cargo(),
        &["clippy", "--", "-D", "warnings"],
        &app,
        &env,
    )?;
    formatting(&based, &app, &env)?;
    failures(&based, &app, &env)?;
    Ok(())
}

fn rename(app: &Path) -> Result<()> {
    let path = app.join("schema/item.bsl");
    let source = fs::read_to_string(&path)?
        .replace("  name: text", "  title: text @was(\"name\")")
        .replace("\n  name\n", "\n  name = title\n")
        .replace("parent { id, name }", "parent { id, name = title }")
        .replace("name = $name", "title = $name");
    fs::write(path, source)?;
    Ok(())
}

async fn rows(app: &Path, column: &str) -> Result<Vec<(String, String, String, Option<String>)>> {
    ensure!(["name", "title"].contains(&column), "invalid lesson column");
    let mut db = initializer::database(app).await?;
    let sql = match column {
        "name" => "SELECT id, name, owner, parent_id FROM item ORDER BY id",
        "title" => "SELECT id, title, owner, parent_id FROM item ORDER BY id",
        _ => unreachable!(),
    };
    Ok(sqlx::query(sql)
        .fetch_all(&mut db)
        .await?
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2), row.get(3)))
        .collect())
}

fn formatting(based: &Path, app: &Path, env: &Environment) -> Result<()> {
    let artifact = app.join("generated/client.rs");
    let before = fs::read(&artifact)?;
    fs::write(
        app.join("rustfmt.toml"),
        "max_width = 60\nuse_small_heuristics = \"Off\"\n",
    )?;
    command::run(command::cargo(), &["fmt"], app, env)?;
    command::run(command::cargo(), &["fmt", "--check"], app, env)?;
    let child = app.join("src/nested");
    fs::create_dir(&child)?;
    command::run(based, &["gen", "all", "--check"], &child, env)?;
    command::run(based, &["gen", "all"], &child, env)?;
    ensure!(
        fs::read(artifact)? == before,
        "formatting/subdirectory generation changed artifact"
    );
    Ok(())
}

fn failures(based: &Path, app: &Path, env: &Environment) -> Result<()> {
    let manifest = app.join("based.toml");
    let before = fs::read(&manifest)?;
    fs::write(&manifest, "dialect='not-a-database'\n")?;
    let error = command::fail(based, &["check"], app, env)?;
    ensure!(
        error.contains("dialect"),
        "invalid configuration error missing"
    );
    fs::write(manifest, before)?;
    let artifact = app.join("generated/client.rs");
    let before = fs::read(&artifact)?;
    fs::write(&artifact, "// User-owned output\n")?;
    let error = command::fail(based, &["gen", "all"], app, env)?;
    ensure!(
        error.contains("--force") && fs::read_to_string(&artifact)? == "// User-owned output\n",
        "output collision lost user source"
    );
    fs::write(artifact, before)?;
    let missing = app.join("absent-directory/database.db");
    command::fail(
        based,
        &[
            "migrate",
            "status",
            "--database-url",
            missing.to_str().unwrap(),
        ],
        app,
        env,
    )?;
    ensure!(!missing.exists(), "failed connection created database");
    Ok(())
}
