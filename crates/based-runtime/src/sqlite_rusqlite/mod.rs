//! The SQLite [`Db`](crate::Db) + [`Backend`](crate::Backend) over rusqlite (feature
//! `sqlite-rusqlite`): the [`crate::sqlite`] backend for an app that already links
//! rusqlite, so both share one `libsqlite3-sys`.
//!
//! Behaviour matches the sqlx backend: the same pool shapes (one pinned in-memory
//! connection; a bounded WAL file pool), binds, row decoding, transactions and error
//! classes. rusqlite is blocking, so each pooled connection lives on its own thread and
//! runs requests in submission order.

mod adopt;
mod backend;
mod begin;
mod bind;
mod db;
mod decode;
mod error;
mod execute;
mod fetch;
mod open;
mod pool;
mod run;
mod tx;
mod value;
mod worker;

#[cfg(test)]
mod tests;

pub use adopt::AdoptedRusqlite;
pub use backend::RusqliteBackend;
pub use db::RusqliteDb;
