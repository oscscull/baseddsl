//! Bind a plan statement for execution: a data-step `create` that omits its primary key
//! binds ids minted now, with the project's id strategy.

use based_codegen::migrate::{IdKind, PlanUnit};

use super::MigrateError;
use crate::value::SqlValue;

/// Mints one id of a kind — the same generator runtime creates use.
pub type Mint = fn(IdKind) -> String;

/// A statement ready to run: its SQL and positional binds.
pub(super) struct Bound {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

/// Bind `unit` (a statement, not a chunked step) for migration `id`.
pub(super) fn bind(unit: &PlanUnit, id: &str, mint: Option<Mint>) -> Result<Bound, MigrateError> {
    match unit {
        PlanUnit::Sql(sql) => Ok(Bound {
            sql: sql.clone(),
            params: Vec::new(),
        }),
        PlanUnit::Minted { sql, ids } => {
            let mint = mint.ok_or_else(|| {
                MigrateError::Artifact(format!(
                    "migration `{id}` creates rows whose ids are minted at apply; pass an id \
                     generator (`ApplyOpts::mint`)"
                ))
            })?;
            let params = ids
                .iter()
                .map(|k| match k {
                    IdKind::Uuid => SqlValue::Uuid(mint(*k)),
                    IdKind::Ulid => SqlValue::Text(mint(*k)),
                })
                .collect();
            Ok(Bound {
                sql: sql.clone(),
                params,
            })
        }
        PlanUnit::Chunked(plan) => Err(MigrateError::Artifact(format!(
            "migration `{id}`: the chunked step on `{}` runs batch by batch",
            plan.table
        ))),
    }
}

/// The production mint: a v4 uuid or a monotonic ULID, as runtime creates use.
#[cfg(feature = "id-gen")]
pub fn production_mint(kind: IdKind) -> String {
    use crate::id::IdGen as _;
    match kind {
        IdKind::Uuid => crate::id::UuidGen.next_id(),
        IdKind::Ulid => crate::id::UuidGen.next_ulid(),
    }
}
