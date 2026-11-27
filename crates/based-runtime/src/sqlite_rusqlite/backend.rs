use async_trait::async_trait;
use rusqlite::Connection;

use super::db::RusqliteDb;
use super::execute::execute;
use super::open;
use super::pool::Pool;
use crate::run::{Backend, Db, DbError, DbRead};
use crate::sqlite_pool_config::SqlitePoolConfig;

/// A pool of one with the default checkout wait.
fn single() -> SqlitePoolConfig {
    SqlitePoolConfig {
        max: 1,
        ..SqlitePoolConfig::default()
    }
}

/// The SQLite [`Backend`] over rusqlite: one bounded pool over one database, no shards
/// (the shard key is ignored). An in-memory backend pins the pool to one connection, so
/// writes persist across checkouts. `Clone` shares the pool.
#[derive(Clone)]
pub struct RusqliteBackend {
    pool: Pool,
}

impl RusqliteBackend {
    /// A backend over a fresh in-memory database: one shared connection, kept for the
    /// backend's life, so one request's writes are visible to the next.
    pub fn in_memory() -> Result<Self, DbError> {
        Ok(Self {
            pool: Pool::new(single(), open::memory()),
        })
    }

    /// A backend over the file database at `path` (created if absent), in WAL mode over the
    /// [`SqlitePoolConfig::default`] pool. [`open_with_config`](Self::open_with_config)
    /// sizes it and bounds the checkout wait.
    pub fn open(path: &str) -> Result<Self, DbError> {
        Self::open_with_config(path, SqlitePoolConfig::default())
    }

    /// [`open`](Self::open) with an explicit maximum pool size (at least 1).
    pub fn open_with_pool(path: &str, max_connections: u32) -> Result<Self, DbError> {
        let cfg = SqlitePoolConfig {
            max: max_connections,
            ..SqlitePoolConfig::default()
        };
        Self::open_with_config(path, cfg)
    }

    /// [`open`](Self::open) with an explicit pool size and checkout wait: a saturated pool
    /// fails a checkout with
    /// [`DbErrorKind::PoolExhausted`](crate::run::DbErrorKind::PoolExhausted) after
    /// `checkout_timeout`.
    pub fn open_with_config(path: &str, cfg: SqlitePoolConfig) -> Result<Self, DbError> {
        Ok(Self {
            pool: Pool::new(cfg, open::file(path)),
        })
    }

    /// Build the backend over the caller's **existing** connection, used exactly as
    /// configured: a pool of one. The app keeps access through
    /// [`with_connection`](Self::with_connection).
    pub fn from_connection(conn: Connection) -> Self {
        Self {
            pool: Pool::new(single(), open::given(conn)),
        }
    }

    /// Build the backend over connections the caller opens and configures, up to
    /// `max_connections` (at least 1) at a time. Each `connect` result is used as is.
    /// A `:memory:` database is per-connection, so an in-memory connector needs a pool of 1.
    pub fn from_connector(
        max_connections: u32,
        connect: impl Fn() -> rusqlite::Result<Connection> + Send + Sync + 'static,
    ) -> Self {
        let open = std::sync::Arc::new(move || connect().map_err(super::error::map_err));
        let cfg = SqlitePoolConfig {
            max: max_connections,
            ..SqlitePoolConfig::default()
        };
        Self {
            pool: Pool::new(cfg, open),
        }
    }

    /// Run `f` on one of the backend's connections, on its connection thread: the app's
    /// own rusqlite access to the database the engine uses.
    pub async fn with_connection<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> R + Send + 'static,
    ) -> Result<R, DbError> {
        self.pool.acquire().await?.call(f).await
    }

    /// Run setup SQL (e.g. `CREATE TABLE …; INSERT …;`): a multi-statement batch.
    pub async fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        let conn = self.pool.acquire().await?;
        execute(&conn, sql, &[]).await.map(|_| ())
    }
}

#[async_trait]
impl Backend for RusqliteBackend {
    async fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(RusqliteDb {
            conn: self.pool.acquire().await?,
        }))
    }

    async fn ping(&self) -> Result<(), DbError> {
        let mut db = RusqliteDb {
            conn: self.pool.acquire().await?,
        };
        crate::run::fetch_all(db.fetch("SELECT 1", &[]))
            .await
            .map(|_| ())
    }
}
