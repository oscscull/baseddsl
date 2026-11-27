//! Pool sizing + checkout wait for the SQLite backends (sqlx and rusqlite).

use std::time::Duration;

/// A SQLite backend's pool: [`PoolConfig`](crate::shard::PoolConfig) minus the server-side
/// statement timeout, which SQLite has no equivalent for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlitePoolConfig {
    /// Maximum open connections (at least 1). An in-memory database is always one.
    pub max: u32,
    /// Max time a checkout waits for a free connection before failing with a
    /// [`DbErrorKind::PoolExhausted`](crate::run::DbErrorKind::PoolExhausted) `503`.
    pub checkout_timeout: Duration,
}

impl Default for SqlitePoolConfig {
    /// A bounded file pool of 5 with the same few-second checkout wait as the server
    /// backends' [`PoolConfig`](crate::shard::PoolConfig).
    fn default() -> Self {
        Self {
            max: 5,
            checkout_timeout: Duration::from_secs(5),
        }
    }
}
