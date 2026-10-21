//! Aggregated SQLite-backed integration suite over the sqlx driver: one test binary instead
//! of a dozen, so the full runtime + sqlx(sqlite) + tokio stack is linked once rather than
//! per file. Each module lives under `tests/sqlite/`; `rusqlite_all` runs the same suite over
//! the rusqlite driver. Gated once here — the whole binary is empty without `sqlite`.
#![cfg(feature = "sqlite")]

#[path = "sqlite/driver_sqlx.rs"]
mod driver;

include!("sqlite/suite.rs");
