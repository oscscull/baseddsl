//! Render the initial offline migration from the checked starter schema.
use crate::error::{io_at, CliError};
use based_codegen::migrate::{self, Snapshot};
use std::path::Path;

pub fn prepare(root: &Path) -> Result<(), CliError> {
    let (project, schema, _, _, _) = crate::project::load_checked(root)?;
    let foreign_keys = based_sema::ForeignKeys::parse(&project.manifest.schema.foreign_keys);
    let snapshot = Snapshot::from_schema_with(&schema, foreign_keys);
    let steps = migrate::diff_snapshots(&Snapshot::default(), &snapshot);
    let dialect = based_codegen::Dialect::parse(&project.manifest.dialect);
    let directory = root.join("migrations/0001_init");
    std::fs::create_dir_all(&directory).map_err(|e| io_at("preparing", &directory, e))?;
    for (name, content) in [
        ("up.mig", migrate::render_up(&steps)),
        ("down.mig", migrate::render_down(&steps, dialect)),
        ("schema.snap", snapshot.render()),
    ] {
        let path = directory.join(name);
        std::fs::write(&path, content).map_err(|e| io_at("preparing", &path, e))?;
    }
    Ok(())
}
