//! Retain declared CHECK constraints without guessing an equivalent application rule.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckConstraint {
    pub name: Option<String>,
    pub expression: String,
}
