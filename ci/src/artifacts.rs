use anyhow::Result;
use std::{fs, path::Path};

pub fn find(target: &Path, consumer: &str, file: &str) -> Result<Vec<std::path::PathBuf>> {
    let build = target.join("debug/build");
    if !build.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(build)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{consumer}-"))
        })
        .map(|entry| entry.path().join("out").join(file))
        .filter(|path| path.is_file())
        .collect())
}
