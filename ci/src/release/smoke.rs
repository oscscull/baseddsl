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
    Ok(())
}
