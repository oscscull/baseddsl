//! A project's `migrations/NNNN_slug/` set: read from disk or supplied as embedded text,
//! and verified offline against the checked schema.
mod files;
mod source;
mod verify;

pub use files::{read_migrations, MigrationFiles};
pub use source::MigrationSource;
pub use verify::{verify_migrations, Finding, Severity};
