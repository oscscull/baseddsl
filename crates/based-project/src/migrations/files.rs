//! Read `<root>/migrations/NNNN_slug/` from disk, ordered by number.
use super::source::{migration_number, MigrationSource};
use crate::Error;
use std::path::{Path, PathBuf};

/// One migration directory read from disk.
#[derive(Debug, Clone)]
pub struct MigrationFiles {
    pub number: u32,
    pub id: String,
    pub dir: PathBuf,
    pub up: String,
    pub down: Option<String>,
    pub snapshot: String,
}

impl MigrationFiles {
    pub fn source(&self) -> MigrationSource<'_> {
        MigrationSource {
            id: &self.id,
            up: &self.up,
            down: self.down.as_deref(),
            snapshot: &self.snapshot,
        }
    }
}

/// The `<root>/migrations/NNNN_slug/` directories, sorted by number. Entries without an
/// `NNNN_` prefix are ignored; a missing `migrations/` directory is an empty set.
pub fn read_migrations(root: &Path) -> Result<Vec<MigrationFiles>, Error> {
    let dir = root.join("migrations");
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let io = |source| Error::Io {
        path: dir.clone(),
        source,
    };
    for entry in std::fs::read_dir(&dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        if !entry.file_type().map_err(io)?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = migration_number(&id) else {
            continue;
        };
        let path = entry.path();
        let down_path = path.join("down.mig");
        out.push(MigrationFiles {
            number,
            id,
            up: read(&path.join("up.mig"))?,
            down: down_path.is_file().then(|| read(&down_path)).transpose()?,
            snapshot: read(&path.join("schema.snap"))?,
            dir: path,
        });
    }
    out.sort_by_key(|m| m.number);
    Ok(out)
}

fn read(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
