use super::*;

/// One returned row: column alias → JSON value (the SELECT aliases each projection
/// to its output name, so a row is already the response object).
pub type Row = serde_json::Map<String, serde_json::Value>;

/// The one read shape: a fallible stream of rows borrowed from the connection it
/// runs on. A one-shot caller collects it ([`fetch_all`]); a streaming caller
/// consumes it row by row.
pub type RowStream<'a> = futures_core::stream::BoxStream<'a, Result<Row, DbError>>;

/// An owned stream of shaped response rows — a `-> stream` query's payload. Each item
/// is exactly one element of what the `[]` form's array would be (nests materialized
/// within the row), in sort order. The stream owns the connection it reads on;
/// dropping it mid-pass cancels the read and returns the connection to the pool
/// (reads hold no transaction). After an `Err` item the stream is finished.
pub type ShapedStream =
    futures_core::stream::BoxStream<'static, Result<serde_json::Value, DbError>>;

/// The read seam a connection and an open transaction share. The runtime hands it
/// positional SQL + values; [`fetch`](DbRead::fetch) streams rows (the *only* read
/// shape — a one-shot caller collects), [`execute`](DbRead::execute) runs one write
/// statement. Every method is fallible: a dependable driver surfaces
/// connection/query failures rather than panicking.
#[async_trait]
pub trait DbRead: Send {
    /// Run a SELECT and stream its rows. The stream borrows the connection; errors
    /// surface as stream items (a failure to even start the query is the first item).
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a>;

    /// Execute one write statement (INSERT/UPDATE/DELETE); returns rows affected.
    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError>;
}

/// A checked-out connection. [`begin`](Db::begin) consumes it into a [`Tx`] — the
/// typestate that makes an open transaction impossible to leak back to the pool.
#[async_trait]
pub trait Db: DbRead {
    /// Open the transaction a mutation body runs in, consuming the connection. Uses the
    /// driver's default isolation — the auto-committing mutation path takes this.
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError>;

    /// Open a transaction at the requested isolation + access mode — the explicit
    /// read-decide-write seam ([`crate::Engine::begin`]). A driver applies the per-dialect
    /// isolation SQL ([`based_codegen::Dialect::begin_transaction_sql`]); the default
    /// ignores `opts` (the mock, and any driver with no isolation control) and opens a
    /// plain transaction.
    async fn begin_tx(self: Box<Self>, opts: crate::tx::TxOptions) -> Result<Box<dyn Tx>, DbError> {
        let _ = opts;
        self.begin().await
    }
}

/// An open transaction. [`commit`](Tx::commit) consumes it; dropping it without
/// commit rolls back or discards the connection (never pooled with an open tx), so a
/// write can only survive via `commit` — cancellation at any await point cannot
/// double-write.
#[async_trait]
pub trait Tx: DbRead {
    async fn commit(self: Box<Self>) -> Result<(), DbError>;

    /// Roll the transaction back explicitly, consuming it. The default drops `self` — the
    /// same rollback the typestate already guarantees on drop; a driver with an awaitable
    /// rollback (all three real drivers) overrides this so the rollback completes before
    /// the connection returns to the pool.
    async fn rollback(self: Box<Self>) -> Result<(), DbError> {
        Ok(())
    }
}

/// A source of per-request database connections, keyed by shard. Given a request's
/// shard key it hands back a boxed [`Db`] to run that request on (single-shard
/// dispatch). This is the seam that keeps the edges driver-neutral: the MariaDB
/// [`crate::driver::ShardRouter`] is one implementation; the Postgres / SQLite
/// backends are others (the [`Db`] trait is already dialect-agnostic — it speaks
/// positional SQL + [`SqlValue`], not a wire protocol).
#[async_trait]
pub trait Backend: Send + Sync {
    /// Check out a connection for the shard the key routes to. A failure (pool
    /// exhausted, shard/host down) is a [`DbError`] → the wire's retryable `503`.
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError>;

    /// Readiness probe: can the backend actually serve traffic *right now*? A
    /// container orchestrator / load balancer calls the listener's `GET /readyz` (which
    /// calls this) before routing traffic to this instance, and pulls it out of
    /// rotation when it fails — so a failure here must mean "don't send me requests"
    /// (every shard's pool is unreachable), not a transient blip.
    ///
    /// The default checks out and returns a connection on the empty shard key (the
    /// common single-shard case): if the pool can hand one out, the backend is ready. A
    /// multi-shard backend overrides this to probe every shard. A backend with no live
    /// database (the mock) is trivially ready.
    async fn ping(&self) -> Result<(), DbError> {
        self.checkout("").await.map(|_| ())
    }
}
