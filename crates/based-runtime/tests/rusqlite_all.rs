//! The SQLite integration suite (`tests/sqlite/`) over the rusqlite driver: the same
//! modules as `sqlite_all`, with `driver` bound to `RusqliteBackend`. Empty without
//! `sqlite-rusqlite`.
#![cfg(feature = "sqlite-rusqlite")]

#[path = "sqlite/driver_rusqlite.rs"]
mod driver;

include!("sqlite/suite.rs");
