//! The selected metadata snapshot and its source identity.
use crate::{CatalogSource, Table};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub source: CatalogSource,
    pub tables: Vec<Table>,
}
