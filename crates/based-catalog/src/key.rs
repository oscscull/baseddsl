//! Declared primary/unique constraints and their ordered physical key columns.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deferral {
    NotDeferrable,
    InitiallyImmediate,
    InitiallyDeferred,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub deferral: Deferral,
}
