//! CLI adapters for compiler projects, backend construction, and facts presentation.
mod backend;
mod discover;
mod facts;
mod load;
mod redaction;

pub use backend::backend;
pub use discover::discover_project;
pub use facts::cmd_facts;
pub use load::{load_checked, Loaded};
pub use redaction::redact;
