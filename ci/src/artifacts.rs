use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Build-script outputs named `file` for `consumer`, in either cargo build-dir layout:
/// `build/<pkg>-<hash>/out` (cargo < 1.100) or `build/<pkg>/<hash>/out` (cargo >= 1.100).
pub fn find(target: &Path, consumer: &str, file: &str) -> Result<Vec<PathBuf>> {
    let build = target.join("debug/build");
    if !build.exists() {
        return Ok(Vec::new());
    }
    let hashed = format!("{consumer}-");
    let mut found = Vec::new();
    for entry in fs::read_dir(build)?.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&hashed) {
            found.push(entry.path().join("out").join(file));
        } else if name == consumer {
            for unit in fs::read_dir(entry.path())?.filter_map(Result::ok) {
                found.push(unit.path().join("out").join(file));
            }
        }
    }
    found.retain(|path| path.is_file());
    Ok(found)
}
