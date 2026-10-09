use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use std::{fs, path::Path};

pub fn check(based: &Path, project: &Path) -> Result<()> {
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(project.join("based.toml"))?)?;
    if manifest
        .get("generate")
        .and_then(|generate| generate.get("client"))
        .is_some()
    {
        command::run(
            based,
            &["gen", "client", "--check"],
            project,
            &Environment::new(),
        )?;
        return Ok(());
    }
    let generated = command::run(
        based,
        &["gen", "client", "--embedded"],
        project,
        &Environment::new(),
    )?;
    ensure!(
        generated.as_bytes() == fs::read(project.join("generated/client.rs"))?,
        "committed client drift"
    );
    Ok(())
}
