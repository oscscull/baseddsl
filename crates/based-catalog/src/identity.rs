//! Preserve physical namespace/name identity without interpreting it as BSL syntax.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QualifiedName {
    pub namespace: String,
    pub name: String,
}

impl QualifiedName {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

/// The physical namespace/table identity used by explicit selection.
pub type TableId = QualifiedName;
