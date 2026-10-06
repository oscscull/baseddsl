//! Standalone store selection and explicit per-database table provisioning.

use crate::error::CliError;
use based_codegen::Dialect;
use based_runtime::{DbStore, IdempotencyStore, MemStore, NoStore};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum StoreMode {
    /// Durable SQL deduplication across instances/restarts; requires a table per shard.
    Database,
    /// Local replay only; entries expire after 24 hours and disappear on restart.
    Memory,
    /// No deduplication: even keyed requests execute on every call.
    None,
}

#[derive(Clone, Copy, Debug, clap::Args)]
pub struct StoreOptions {
    /// Store for keyed mutations. Database retains keys until explicitly deleted.
    #[arg(
        long,
        env = "BASED_IDEMPOTENCY_STORE",
        value_enum,
        default_value = "database"
    )]
    pub idempotency_store: StoreMode,
    /// Explicitly create the idempotency table on every configured database/shard.
    /// Requires CREATE TABLE permission; otherwise provision it through a migration.
    #[arg(long, env = "BASED_INIT_IDEMPOTENCY_TABLE")]
    pub init_idempotency_table: bool,
}

impl StoreOptions {
    pub fn validate(self) -> Result<(), CliError> {
        if self.init_idempotency_table && !matches!(self.idempotency_store, StoreMode::Database) {
            return Err(CliError::usage(
                "--init-idempotency-table requires --idempotency-store database",
            ));
        }
        Ok(())
    }

    pub fn uses_database(self) -> bool {
        matches!(self.idempotency_store, StoreMode::Database)
    }

    pub fn build(self, dialect: Dialect) -> Box<dyn IdempotencyStore> {
        match self.idempotency_store {
            StoreMode::Database => {
                eprintln!("idempotency: database (atomic SQL deduplication; keys retained until explicitly deleted)");
                Box::new(DbStore::new(dialect))
            }
            StoreMode::Memory => {
                eprintln!(
                    "idempotency: memory (process-local replay; 24-hour TTL; lost on restart)"
                );
                Box::new(MemStore::new())
            }
            StoreMode::None => {
                eprintln!("idempotency: none (keyed mutations may execute repeatedly)");
                Box::new(NoStore)
            }
        }
    }
}
