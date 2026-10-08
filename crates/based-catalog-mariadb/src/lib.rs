//! MariaDB metadata discovery; no application-row queries or model emission.
#![cfg(feature = "mariadb")]
mod checks;
mod column_limits;
mod columns;
mod connection;
mod definition;
mod discovery;
mod foreign_keys;
mod indexes;
mod keys;
mod limitations;
mod snapshot;
mod table;
mod types;
mod visibility;

pub use connection::MariaDbCatalogReader;
