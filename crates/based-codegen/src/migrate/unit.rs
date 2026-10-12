//! One executable unit of a migration direction, as the runtime runs it.

use super::data::ChunkPlan;

/// A statement, a statement binding ids minted at apply time, or a chunked data step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanUnit {
    Sql(String),
    /// `sql` binds one freshly minted id per entry of `ids`, in placeholder order.
    Minted {
        sql: String,
        ids: Vec<IdKind>,
    },
    Chunked(ChunkPlan),
}

/// The project id strategy a minted primary key follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Uuid,
    Ulid,
}
