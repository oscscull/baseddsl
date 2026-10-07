//! Context and operator guidance for a failed migration execution.
use crate::run::DbError;
use based_codegen::Dialect;

#[derive(Debug, Clone)]
pub struct MigrationFailure {
    pub id: String,
    pub direction: &'static str,
    pub stage: String,
    pub dialect: Dialect,
    pub source: DbError,
}

impl std::fmt::Display for MigrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "migration `{}` ({}) failed at {}: {}; earlier completed migrations remain applied; ",
            self.id, self.direction, self.stage, self.source.message
        )?;
        if matches!(self.dialect, Dialect::MariaDb | Dialect::MySql) {
            return f.write_str("DDL may already be committed even without a completion ledger row; inspect the live schema and _based_migrations, restore a known state before retry (docs/migration-recovery.md)");
        }
        f.write_str("transaction rollback is expected for transactional SQL, but commit failures can be ambiguous; inspect the live schema and _based_migrations before retry (docs/migration-recovery.md)")
    }
}
