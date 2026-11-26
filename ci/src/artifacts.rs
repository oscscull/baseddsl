use anyhow::Result;
use std::path::{Path, PathBuf};

/// Build-script `OUT_DIR` files for `consumer`, under either Cargo build-dir layout:
/// `build/<pkg>-<hash>/out` (before 1.100) or `build/<pkg>/<hash>/out`.
pub fn find(target: &Path, consumer: &str, file: &str) -> Result<Vec<PathBuf>> {
    let build = target.join("debug/build");
    if !build.exists() {
        return Ok(Vec::new());
    }
    let prefix = format!("{consumer}-");
    let mut found = Vec::new();
    for entry in walkdir::WalkDir::new(&build).min_depth(1).max_depth(1) {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy();
        let roots: Vec<PathBuf> = if name.starts_with(&prefix) {
            vec![entry.path().to_path_buf()]
        } else if name == consumer {
            walkdir::WalkDir::new(entry.path())
                .min_depth(1)
                .max_depth(1)
                .into_iter()
                .filter_map(Result::ok)
                .map(walkdir::DirEntry::into_path)
                .collect()
        } else {
            continue;
        };
        found.extend(
            roots
                .into_iter()
                .map(|root| root.join("out").join(file))
                .filter(|path| path.is_file()),
        );
    }
    Ok(found)
}
