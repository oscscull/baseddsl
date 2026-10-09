use anyhow::{ensure, Result};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

pub fn verify(path: &Path, version: &str) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let mut manifest = String::new();
    archive
        .by_name("extension/package.json")?
        .read_to_string(&mut manifest)?;
    let manifest: Value = serde_json::from_str(&manifest)?;
    ensure!(
        manifest["version"] == version && manifest["license"] == "AGPL-3.0-only",
        "VSIX metadata mismatch"
    );
    for name in [
        "extension/out/extension.js",
        "extension/node_modules/vscode-languageclient/lib/node/main.js",
    ] {
        ensure!(
            archive.by_name(name).is_ok(),
            "missing VSIX runtime file: {name}"
        );
    }
    ensure!(
        !archive
            .file_names()
            .any(|name| name.starts_with("extension/test/")),
        "VSIX includes tests"
    );
    Ok(())
}
