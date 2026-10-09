//! Supply context from a trusted host identity; this demo has no login system.
use crate::client::Uuid;

pub struct Session {
    owner: Uuid,
}

impl Session {
    pub fn local_demo() -> Self {
        Self {
            owner: "00000000-0000-4000-8000-000000000001".into(),
        }
    }

    pub fn other_demo() -> Self {
        Self {
            owner: "00000000-0000-4000-8000-000000000002".into(),
        }
    }

    pub fn owner(&self) -> Uuid {
        self.owner.clone()
    }
}
