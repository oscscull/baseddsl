use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use serde_json::Value;
use sqlx::{Connection, Row, SqliteConnection};
use std::{collections::BTreeMap, fs, path::Path};

pub fn identity(output: &str) -> Result<()> {
    let record = |label: &str| -> Result<Value> {
        let text = output
            .lines()
            .find_map(|line| line.strip_prefix(label))
            .ok_or_else(|| anyhow::anyhow!("missing {label}: {output}"))?;
        Ok(serde_json::from_str(text.trim())?)
    };
    let created = record("created:")?;
    let rows = record("read:")?;
    let id = created["id"].as_str().unwrap_or_default();
    ensure!(
        id.len() == 36 && id.as_bytes()[14] == b'4',
        "expected production UUID: {created}"
    );
    let rows = rows
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("read did not return rows"))?;
    ensure!(
        rows.iter().any(|row| row["id"] == created["id"]),
        "created row absent"
    );
    ensure!(
        created["parent"]["name"] == "Parent",
        "nested parent missing"
    );
    ensure!(
        rows.iter().any(|row| row["id"] == created["parent"]["id"]),
        "parent row absent"
    );
    Ok(())
}

pub fn fresh(based: &Path, app: &Path, mode: &str, dialect: &str, env: &Environment) -> Result<()> {
    let instructions = command::run(
        based,
        &[
            "init",
            app.to_str().unwrap(),
            "--mode",
            mode,
            "--dialect",
            dialect,
        ],
        &command::root(),
        env,
    )?;
    ensure!(
        instructions.contains("1. based migrate apply") && instructions.contains("2. cargo run"),
        "wrong next steps: {instructions}"
    );
    ensure!(
        !app.join("local.db").exists() && !app.join("build.rs").exists(),
        "initializer performed runtime work"
    );
    command::run(based, &["gen", "all", "--check"], app, env)?;
    command::run(based, &["migrate", "verify"], app, env)?;
    let before = fingerprint(app)?;
    let error = command::fail(
        based,
        &["init", app.to_str().unwrap(), "--mode", mode],
        &command::root(),
        env,
    )?;
    ensure!(
        error.contains("empty directory") && before == fingerprint(app)?,
        "rerun changed existing project"
    );
    Ok(())
}

pub async fn database(app: &Path) -> Result<SqliteConnection> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(app.join("local.db"))
        .read_only(true)
        .create_if_missing(false);
    Ok(SqliteConnection::connect_with(&options).await?)
}

async fn columns(app: &Path) -> Result<Vec<String>> {
    let mut db = database(app).await?;
    Ok(sqlx::query("PRAGMA table_info(Item)")
        .fetch_all(&mut db)
        .await?
        .iter()
        .map(|row| row.get("name"))
        .collect())
}

pub async fn verify(based: &Path) -> Result<()> {
    let based = based.canonicalize()?;
    let scratch = tempfile::tempdir()?;
    let mut env = crate::source::mirror(scratch.path())?;
    env.insert("BASED".into(), based.display().to_string());
    for mode in ["embedded", "standalone"] {
        let app = scratch.path().join(format!("{mode}-sqlite"));
        fresh(&based, &app, mode, "sqlite", &env)?;
        command::run(
            &based,
            &["migrate", "apply", "--database-url", "local.db"],
            &app,
            &env,
        )?;
        command::run(command::cargo(), &["fmt", "--check"], &app, &env)?;
        let output = command::run(command::cargo(), &["run"], &app, &env)?;
        identity(&output)?;
        if mode == "embedded" {
            ensure!(
                output.contains("lookup: not found in this owner's context"),
                "owner lookup escaped scope"
            );
        }
        crate::files::replace(
            &app.join("schema/item.bsl"),
            "  name: text",
            "  name: text\n  description: text?",
        )?;
        for args in [
            &["gen", "all"][..],
            &["migrate", "gen"],
            &["migrate", "verify"],
        ] {
            command::run(&based, args, &app, &env)?;
        }
        ensure!(
            !columns(&app)
                .await?
                .iter()
                .any(|name| name == "description"),
            "generation applied migration"
        );
        command::run(
            &based,
            &["migrate", "apply", "--database-url", "local.db"],
            &app,
            &env,
        )?;
        ensure!(
            columns(&app)
                .await?
                .iter()
                .any(|name| name == "description"),
            "migration not applied"
        );
        identity(&command::run(command::cargo(), &["run"], &app, &env)?)?;
        command::run(
            command::cargo(),
            &["clippy", "--", "-D", "warnings"],
            &app,
            &env,
        )?;
        for dialect in ["mariadb", "postgres"] {
            let app = scratch.path().join(format!("{mode}-{dialect}"));
            fresh(&based, &app, mode, dialect, &env)?;
            command::run(command::cargo(), &["fmt", "--check"], &app, &env)?;
            command::run(
                command::cargo(),
                &["clippy", "--", "-D", "warnings"],
                &app,
                &env,
            )?;
        }
    }
    collisions(&based, scratch.path(), &env)?;
    Ok(())
}

fn fingerprint(app: &Path) -> Result<BTreeMap<std::path::PathBuf, Vec<u8>>> {
    walkdir::WalkDir::new(app)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "target")
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_file() => Some(Ok(entry)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .map(|entry| {
            let entry = entry?;
            Ok((
                entry.path().strip_prefix(app)?.to_path_buf(),
                fs::read(entry.path())?,
            ))
        })
        .collect()
}

fn collisions(based: &Path, root: &Path, env: &Environment) -> Result<()> {
    let existing = root.join("existing-rust");
    fs::create_dir(&existing)?;
    fs::write(
        existing.join("Cargo.toml"),
        "[package]\nname='existing'\nversion='0.1.0'\n",
    )?;
    let before = fingerprint(&existing)?;
    let error = command::fail(
        based,
        &["init", existing.to_str().unwrap(), "--mode", "embedded"],
        root,
        env,
    )?;
    ensure!(
        error.contains("existing Rust projects") && fingerprint(&existing)? == before,
        "existing project changed"
    );
    let file = root.join("source-file");
    fs::write(&file, "preserve source")?;
    command::fail(
        based,
        &["init", file.to_str().unwrap(), "--mode", "embedded"],
        root,
        env,
    )?;
    ensure!(
        fs::read_to_string(file)? == "preserve source",
        "source file overwritten"
    );
    #[cfg(unix)]
    {
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&existing, &alias)?;
        command::fail(
            based,
            &["init", alias.to_str().unwrap(), "--mode", "standalone"],
            root,
            env,
        )?;
        ensure!(
            fingerprint(&existing)? == before,
            "symlink target overwritten"
        );
    }
    Ok(())
}
