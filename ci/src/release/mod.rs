mod archive;
mod checksums;
mod extension;
mod metadata;
mod smoke;

use anyhow::{ensure, Result};
use serde_json::Value;
use std::{fs, path::Path};

pub async fn package(
    binary_dir: &Path,
    output: &Path,
    target: &str,
    tag: Option<&str>,
    allow_dirty: bool,
) -> Result<()> {
    let mut source = metadata::source(tag, allow_dirty)?;
    let rustc = crate::command::run(
        "rustc",
        &["-Vv"],
        &crate::command::root(),
        &crate::command::Environment::new(),
    )?;
    let host = rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or_else(|| anyhow::anyhow!("missing rustc host"))?;
    ensure!(host == target, "package and smoke on the native target");
    source["toolchain"] = serde_json::json!({"rustc":rustc.trim(), "target":host});
    let artifact = archive::package(binary_dir, output, target, &source)?;
    smoke::verify(&artifact).await?;
    checksums::write(output)?;
    println!("{}", artifact.display());
    Ok(())
}

pub fn extension(directory: &Path, output: &Path) -> Result<()> {
    let source = metadata::source(None, false)?;
    let name = format!("based-vscode-{}.vsix", source["version"].as_str().unwrap());
    extension::verify(&directory.join(&name), source["version"].as_str().unwrap())?;
    fs::create_dir_all(output)?;
    fs::copy(directory.join(&name), output.join(&name))?;
    Ok(())
}

pub fn collect(directory: &Path, expected_count: usize) -> Result<()> {
    let source = metadata::source(None, false)?;
    let version = source["version"].as_str().unwrap();
    let prefix = format!("based-{version}-");
    let archives = fs::read_dir(directory)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .collect::<Vec<_>>();
    ensure!(
        archives.len() == expected_count,
        "expected {expected_count} native archives, got {}",
        archives.len()
    );
    let mut targets = std::collections::BTreeSet::new();
    for artifact in archives {
        let scratch = tempfile::tempdir()?;
        archive::extract(&artifact, scratch.path())?;
        let record: Value =
            serde_json::from_slice(&fs::read(scratch.path().join("manifest.json"))?)?;
        ensure!(
            record["version"] == source["version"]
                && record["commit"] == source["commit"]
                && record["dirty"] == false,
            "mismatched release source: {}",
            artifact.display()
        );
        ensure!(
            targets.insert(record["target"].as_str().unwrap().to_owned()),
            "duplicate target"
        );
    }
    let extensions = fs::read_dir(directory)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "vsix"))
        .collect::<Vec<_>>();
    ensure!(extensions.len() == 1, "expected one matching VSIX");
    extension::verify(&extensions[0], version)?;
    checksums::write(directory)?;
    Ok(())
}
