//! One migration's artifacts as text, independent of where they were read from.

/// One `migrations/NNNN_slug/` directory's files: `up.mig`, the optional `down.mig`, and
/// `schema.snap`. `id` is the directory name, which orders the set and keys the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationSource<'a> {
    pub id: &'a str,
    pub up: &'a str,
    pub down: Option<&'a str>,
    pub snapshot: &'a str,
}

impl MigrationSource<'_> {
    /// The `NNNN` ordinal of an `NNNN_slug` id; `None` when the id has no numeric prefix.
    pub fn number(&self) -> Option<u32> {
        migration_number(self.id)
    }
}

/// The `NNNN` ordinal of an `NNNN_slug` directory name.
pub(super) fn migration_number(name: &str) -> Option<u32> {
    name.split_once('_')?.0.parse().ok()
}
