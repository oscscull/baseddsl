use anyhow::{ensure, Result};
use std::path::Path;

pub fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(source)
        .into_iter()
        .filter_entry(|e| e.file_name() != "target" && e.file_name() != ".git")
    {
        let entry = entry?;
        let path = target.join(entry.path().strip_prefix(source)?);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(path)?;
            continue;
        }
        std::fs::copy(entry.path(), path)?;
    }
    Ok(())
}

pub fn replace(path: &Path, before: &str, after: &str) -> Result<()> {
    let source = std::fs::read_to_string(path)?;
    ensure!(
        source.contains(before),
        "missing replacement input in {}",
        path.display()
    );
    std::fs::write(path, source.replace(before, after))?;
    Ok(())
}

pub fn executable(path: &Path, content: &str) -> Result<()> {
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
