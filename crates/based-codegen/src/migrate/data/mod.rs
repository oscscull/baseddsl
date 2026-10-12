//! Typed migration data steps (`create` / `update` / `delete` lines in an `up.mig`): checked
//! offline against the schema state at their line, then lowered per dialect.

mod check;
mod invert;
mod lower;
mod relation;
mod table;

pub(crate) use check::check_step;
pub use check::DataError;
pub(crate) use invert::invert;
pub(crate) use lower::lower_step;
pub use lower::ChunkPlan;
