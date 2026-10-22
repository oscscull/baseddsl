//! Run the embedded tutorial in a clean external project with the given binary.
mod tutorial;

use crate::{
    command::{self, Environment},
    initializer,
};
use anyhow::{ensure, Result};
use sqlx::Row;
use std::{fs, path::Path};

pub async fn verify(based: &Path) -> Result<()> {
    tutorial::require_documented(&command::root().join("book/src/embedded.md"))?;
    let based = based.canonicalize()?;
    let scratch = tempfile::tempdir()?;
    let env = crate::source::mirror(scratch.path())?;
    run(&based, tutorial::INIT, scratch.path(), &env)?;
    let app = scratch.path().join("items");
    ensure!(
        fs::read(app.join("schema/item.bsl"))? == fs::read(tutorial::starter_schema())?,
        "initializer schema differs from the tutorial's"
    );
    let manifest = fs::read_to_string(app.join("Cargo.toml"))?;
    ensure!(
        manifest.contains("git = \"https://github.com/oscscull/baseddsl.git\"")
            && !manifest.contains("path ="),
        "consumer did not use pinned public Git source"
    );
    run(&based, tutorial::MIGRATE, &app, &env)?;
    initializer::identity(&run(&based, tutorial::RUN, &app, &env)?)?;
    ensure!(
        fs::read_to_string(app.join("Cargo.lock"))?
            .contains(&format!("#{}", command::git(&["rev-parse", "HEAD"])?)),
        "consumer resolved another commit"
    );
    let before = rows(&app, "name").await?;
    ensure!(before.len() == 2, "wrong lesson rows");
    fs::copy(tutorial::renamed_schema(), app.join("schema/item.bsl"))?;
    for args in tutorial::EVOLVE {
        run(&based, args, &app, &env)?;
    }
    ensure!(
        rows(&app, "name").await? == before,
        "generation changed data"
    );
    run(&based, tutorial::MIGRATE, &app, &env)?;
    ensure!(
        rows(&app, "title").await? == before,
        "rename changed rows or relationship IDs"
    );
    initializer::identity(&run(&based, tutorial::RUN, &app, &env)?)?;
    reset(&based, &app, &env).await?;
    command::run(command::cargo(), &["fmt", "--check"], &app, &env)?;
    command::run(
        command::cargo(),
        &["clippy", "--", "-D", "warnings"],
        &app,
        &env,
    )?;
    formatting(&app, &env)?;
    failures(&based, &app, &env)?;
    Ok(())
}

/// The documented reset leaves a fresh database that one run fills with one pair again.
async fn reset(based: &Path, app: &Path, env: &Environment) -> Result<()> {
    run(based, tutorial::RESET, app, env)?;
    ensure!(!app.join("local.db").exists(), "reset left the database");
    run(based, tutorial::MIGRATE, app, env)?;
    run(based, tutorial::RUN, app, env)?;
    ensure!(rows(app, "title").await?.len() == 2, "reset kept old rows");
    Ok(())
}

/// Run a tutorial command, with `based` as the binary under test.
fn run(based: &Path, args: &[&str], cwd: &Path, env: &Environment) -> Result<String> {
    match args[0] {
        "based" => command::run(based, &args[1..], cwd, env),
        "cargo" => command::run(command::cargo(), &args[1..], cwd, env),
        program => command::run(program, &args[1..], cwd, env),
    }
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

fn formatting(app: &Path, env: &Environment) -> Result<()> {
    let target = Path::new(&env["CARGO_TARGET_DIR"]);
    let clients = crate::artifacts::find(target, "based-starter", "client.rs")?;
    ensure!(!clients.is_empty(), "missing OUT_DIR client");
    let before = clients
        .iter()
        .map(fs::read)
        .collect::<Result<Vec<_>, _>>()?;
    fs::write(
        app.join("rustfmt.toml"),
        "max_width = 60\nuse_small_heuristics = \"Off\"\n",
    )?;
    command::run(command::cargo(), &["fmt"], app, env)?;
    command::run(command::cargo(), &["fmt", "--check"], app, env)?;
    ensure!(
        clients
            .iter()
            .map(fs::read)
            .collect::<Result<Vec<_>, _>>()?
            == before,
        "formatting changed generated client"
    );
    ensure!(
        !walkdir::WalkDir::new(app)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != "target")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == "client.rs"),
        "generated client escaped OUT_DIR"
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
    let schema = app.join("schema/item.bsl");
    let before = fs::read(&schema)?;
    fs::write(&schema, "Item { id: serial, name:\n")?;
    let error = command::fail(command::cargo(), &["build"], app, env)?;
    ensure!(
        error.contains("item.bsl:") && error.contains("check failed:"),
        "build did not surface the schema diagnostic: {error}"
    );
    fs::write(&schema, before)?;
    command::run(command::cargo(), &["build"], app, env)?;
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
