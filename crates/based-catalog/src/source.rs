//! Identify the database family/version without retaining a connection URL or secret.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogDialect {
    MariaDb,
    Postgres,
    Sqlite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSource {
    pub dialect: CatalogDialect,
    pub server_version: String,
    pub database: String,
}
