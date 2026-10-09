//! PostgreSQL metadata discovery; no application-row reads or model emission.
#![cfg(feature = "postgres")]
mod checks;
mod column_limits;
mod columns;
mod connection;
mod constraint_limits;
mod constraints;
mod discovery;
mod foreign_keys;
mod generation;
mod generation_limits;
mod index_limits;
mod indexes;
mod keys;
mod table;
mod table_limits;
mod type_modifiers;
mod types;

pub use connection::PostgresCatalogReader;
