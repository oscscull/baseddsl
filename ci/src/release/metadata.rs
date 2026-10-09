use crate::command::{self, Environment};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub fn source(tag: Option<&str>, allow_dirty: bool) -> Result<Value> {
    let root = command::root();
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let package = &manifest["workspace"]["package"];
    let version = package["version"].as_str().unwrap();
    if let Some(tag) = tag {
        ensure!(
            tag == format!("v{version}"),
            "tag does not match source version"
        );
    }
    let extension: Value =
        serde_json::from_slice(&fs::read(root.join("editors/vscode/package.json"))?)?;
    let lock: Value =
        serde_json::from_slice(&fs::read(root.join("editors/vscode/package-lock.json"))?)?;
    ensure!(
        extension["version"] == version
            && lock["version"] == version
            && lock["packages"][""]["version"] == version,
        "component versions differ"
    );
    for entry in fs::read_dir(root.join("crates"))? {
        let path = entry?.path().join("Cargo.toml");
        if !path.is_file() {
            continue;
        }
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
        for key in ["version", "license", "rust-version"] {
            ensure!(
                manifest["package"][key]["workspace"].as_bool() == Some(true),
                "{} does not inherit {key}",
                path.display()
            );
        }
    }
    let dirty = !command::git(&["status", "--porcelain"])?.is_empty();
    ensure!(
        allow_dirty || !dirty,
        "release source is dirty; --allow-dirty is for local dry runs"
    );
    Ok(
        json!({"version":version, "commit":command::git(&["rev-parse", "HEAD"])?, "dirty":dirty,
        "license":package["license"].as_str(), "rust_version":package["rust-version"].as_str(), "tag":tag}),
    )
}

pub fn binary(binary: &Path, source: &Value) -> Result<String> {
    let output = command::run(
        binary,
        &["--version"],
        &command::root(),
        &Environment::new(),
    )?;
    let identity = output.trim();
    let version = source["version"].as_str().unwrap();
    let commit = source["commit"].as_str().unwrap();
    ensure!(
        identity.contains(&format!(" {version} (")) && identity.contains(&commit[..12]),
        "binary/source identity mismatch: {identity}"
    );
    ensure!(
        source["dirty"] == true || !identity.contains("-dirty"),
        "dirty release binary"
    );
    Ok(identity.into())
}
