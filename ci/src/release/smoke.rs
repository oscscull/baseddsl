use super::{archive, metadata};
use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::{fs, path::Path};

pub async fn verify(artifact: &Path) -> Result<()> {
    let scratch = tempfile::tempdir()?;
    let installed = scratch.path().join("installed");
    archive::extract(artifact, &installed)?;
    let source: Value = serde_json::from_slice(&fs::read(installed.join("manifest.json"))?)?;
    let suffix = if source["target"].as_str().unwrap().contains("windows") {
        ".exe"
    } else {
        ""
    };
    let based = installed.join(format!("based{suffix}"));
    let lsp = installed.join(format!("based-lsp{suffix}"));
    metadata::binary(&based, &source)?;
    metadata::binary(&lsp, &source)?;
    let env = Environment::new();
    command::run(&lsp, &["--help"], scratch.path(), &env)?;
    let app = scratch.path().join("project");
    fs::create_dir_all(app.join("schema"))?;
    fs::write(
        app.join("based.toml"),
        "dialect='sqlite'\nroot='schema'\n[generate]\nclient='generated/client.rs'\n",
    )?;
    fs::write(
        app.join("schema/item.bsl"),
        "Item { id: Id, name: text }\nquery items() -> Item[] { list Item order (id); }\n",
    )?;
    for args in [
        &["check"][..],
        &["gen", "all"],
        &["gen", "all", "--check"],
        &["migrate", "gen"],
        &["migrate", "verify"],
        &["migrate", "apply", "--database-url", "local.db"],
    ] {
        command::run(&based, args, &app, &env)?;
    }
    let status = command::run(
        &based,
        &["migrate", "status", "--database-url", "local.db"],
        &app,
        &env,
    )?;
    ensure!(
        !status.to_lowercase().contains("pending") || status.to_lowercase().contains("0 pending"),
        "pending native migration: {status}"
    );
    sqlite_engine(&based, scratch.path(), &app.join("local.db"))?;
    Ok(())
}

fn sqlite_engine(based: &Path, root: &Path, database: &Path) -> Result<()> {
    let probe = root.join("import-probe");
    fs::create_dir_all(probe.join("models"))?;
    fs::write(
        probe.join("based.toml"),
        "dialect='sqlite'\nroot='models'\n",
    )?;
    let before = fs::read(database)?;
    let output = command::run(
        based,
        &[
            "import",
            "--database-url",
            database.to_str().unwrap(),
            "--table",
            "main.item",
            "--json",
        ],
        &probe,
        &Environment::new(),
    )?;
    let report: Value = serde_json::from_str(&output)?;
    let version = report["catalog"]["source"]["server_version"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing SQLite version"))?;
    let version = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        version.as_slice() >= [3, 51, 3].as_slice(),
        "extracted CLI uses an older SQLite engine"
    );
    ensure!(
        report["status"] == "imported"
            && fs::read(database)? == before
            && !probe.join("migrations").exists(),
        "native import changed database"
    );
    Ok(())
}
