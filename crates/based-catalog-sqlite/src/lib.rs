//! SQLite file catalog discovery with read-only opening and denied application-row reads.
#![cfg(feature = "sqlite")]
mod authorizer;
mod collisions;
mod columns;
mod connection;
mod discovery;
mod foreign_keys;
mod indexes;
mod keys;
mod keywords;
mod limitations;
mod opaque;
mod rowid;
mod table;
mod types;

pub use connection::SqliteCatalogReader;

#[cfg(test)]
mod access_tests;
