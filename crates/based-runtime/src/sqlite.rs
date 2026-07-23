//! The SQLite [`Db`] + [`Backend`] (feature `sqlite`), over sqlx's async SQLite driver.
//!
//! SQLite is the infra-free backend: an in-memory (or file) database that needs no live
//! server, so it unlocks real end-to-end integration tests against a genuine engine — the
//! whole `plan → run → shape` path exercised over an actual `Db`, not a `MockDb` (see
//! `tests/sqlite_integration.rs`). It binds positional `?` exactly like MariaDB (no
//! dialect-aware scanner change, unlike Postgres's `$n`), and accepts the DML the runtime
//! executes (backtick-quoted identifiers, `= TRUE`, `IS NULL`, `LIMIT`, multi-table joins)
//! as-is.
//!
//! Structure mirrors the other drivers: [`SqliteDb`] (one pooled connection), a typestate
//! transaction guard, and [`SqliteBackend`] — a single database has no shards, so the
//! backend ignores the shard key. An **in-memory** backend pins its pool to exactly one
//! connection that never expires: SQLite's `:memory:` database is per-connection, so the
//! single recycled connection is what lets separate requests see each other's writes
//! (checkouts queue on it — correct for SQLite, which serializes writers anyway).

use async_trait::async_trait;
use futures_util::StreamExt;
use sqlx::pool::PoolConnection;
use sqlx::query::Query;
use sqlx::sqlite::{Sqlite, SqliteArguments, SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{Column, Row as SqlxRow, TypeInfo, ValueRef};

use crate::run::{Backend, Db, DbError, DbErrorKind, DbRead, Row, RowStream, Tx};
use crate::value::SqlValue;

/// How long a locked SQLite database waits for the lock to clear before returning
/// `SQLITE_BUSY`. SQLite serializes writers, so under a concurrent writer (another process
/// on a file DB) a statement would otherwise fail instantly; the busy timeout lets it wait
/// a bounded moment, and a genuine timeout then surfaces as a retryable deadlock.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Map a sqlx error to the runtime's [`DbError`] (→ the wire's retryable `503`). A
/// `SQLITE_BUSY` (5) / `SQLITE_LOCKED` (6) — a writer lost the lock race after the busy
/// timeout — is a deadlock-class failure the mutation path may retry; everything else is
/// opaque.
fn map_sqlite_err(e: sqlx::Error) -> DbError {
    let kind = match e
        .as_database_error()
        .and_then(|d| d.code())
        .and_then(|c| c.parse::<i64>().ok())
    {
        // Extended result codes carry the primary code in the low byte.
        Some(code) if code & 0xff == 5 || code & 0xff == 6 => DbErrorKind::Deadlock,
        _ => DbErrorKind::Other,
    };
    DbError::of(kind, e.to_string())
}

/// Bind every [`SqlValue`] onto a query, positionally. A `bool` binds as SQLite's integer
/// `0/1`; every text-riding family binds its wire string (SQLite stores them as `TEXT`,
/// which is how the wire reads them back); `json` is serialized text.
fn bind_all<'q>(
    mut q: Query<'q, Sqlite, SqliteArguments>,
    params: &[SqlValue],
) -> Query<'q, Sqlite, SqliteArguments> {
    for v in params {
        q = match v {
            SqlValue::Null => q.bind(Option::<String>::None),
            SqlValue::Int(i) => q.bind(*i),
            SqlValue::Float(f) => q.bind(*f),
            SqlValue::Bool(b) => q.bind(*b as i64),
            SqlValue::Text(s)
            | SqlValue::Uuid(s)
            | SqlValue::Timestamp(s)
            | SqlValue::Date(s)
            | SqlValue::Decimal(s) => q.bind(s.clone()),
            SqlValue::Json(j) => q.bind(j.to_string()),
        };
    }
    q
}

/// One result row → JSON (the shape the wire response is built from), each column read
/// by its *value's* storage class (SQLite types per value, not per column). Text/uuid/
/// json/timestamp/decimal all ride the wire as strings, so `TEXT` maps straight through;
/// a genuinely binary `BLOB` falls back to lowercase hex (never a panic).
fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> Result<Row, DbError> {
    use serde_json::Value as J;
    let mut obj = serde_json::Map::with_capacity(row.columns().len());
    for (i, col) in row.columns().iter().enumerate() {
        let raw = row.try_get_raw(i).map_err(map_sqlite_err)?;
        let val = if raw.is_null() {
            J::Null
        } else {
            let ty = raw.type_info().name().to_string();
            decode_sqlite(row, i, &ty)
                .map_err(|e| DbError::new(format!("decoding column `{}`: {e}", col.name())))?
        };
        obj.insert(col.name().to_string(), val);
    }
    Ok(obj)
}

fn decode_sqlite(
    row: &sqlx::sqlite::SqliteRow,
    i: usize,
    ty: &str,
) -> Result<serde_json::Value, sqlx::Error> {
    use serde_json::Value as J;
    Ok(match ty {
        "INTEGER" | "BOOLEAN" => J::Number(row.try_get_unchecked::<i64, _>(i)?.into()),
        "REAL" => serde_json::Number::from_f64(row.try_get_unchecked::<f64, _>(i)?)
            .map_or(J::Null, J::Number),
        "BLOB" => J::String(hex(&row.try_get_unchecked::<Vec<u8>, _>(i)?)),
        // TEXT (and anything else stringlike): the stored string, verbatim.
        _ => J::String(row.try_get_unchecked::<String, _>(i)?),
    })
}

/// Lowercase hex of a byte slice (for a blob column value).
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// One pooled SQLite connection, running one request.
pub struct SqliteDb {
    conn: PoolConnection<Sqlite>,
}

#[async_trait]
impl DbRead for SqliteDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let q = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params);
        Box::pin(
            q.fetch(&mut *self.conn)
                .map(|r| r.map_err(map_sqlite_err).and_then(|row| row_to_json(&row))),
        )
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params)
            .execute(&mut *self.conn)
            .await
            .map(|d| d.rows_affected())
            .map_err(map_sqlite_err)
    }
}

#[async_trait]
impl Db for SqliteDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        let tx = sqlx::Transaction::begin(self.conn, None)
            .await
            .map_err(map_sqlite_err)?;
        Ok(Box::new(SqliteTx { tx }))
    }
}

/// An open SQLite transaction (sqlx's guard over the same pooled connection).
struct SqliteTx {
    tx: sqlx::Transaction<'static, Sqlite>,
}

#[async_trait]
impl DbRead for SqliteTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let q = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params);
        Box::pin(
            q.fetch(&mut *self.tx)
                .map(|r| r.map_err(map_sqlite_err).and_then(|row| row_to_json(&row))),
        )
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params)
            .execute(&mut *self.tx)
            .await
            .map(|d| d.rows_affected())
            .map_err(map_sqlite_err)
    }
}

#[async_trait]
impl Tx for SqliteTx {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        self.tx.commit().await.map_err(map_sqlite_err)
    }
}

/// The SQLite [`Backend`]: one bounded pool over one database, no shards (the shard key
/// is ignored). An in-memory backend pins the pool to a single never-expiring connection
/// so writes persist across checkouts — the property that makes it a real
/// integration-test engine.
pub struct SqliteBackend {
    pool: SqlitePool,
}

impl SqliteBackend {
    /// A backend over a fresh in-memory database (`:memory:`). One shared connection,
    /// recycled forever, so writes from one request are visible to the next — no infra,
    /// no file.
    pub fn in_memory() -> Result<SqliteBackend, DbError> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            // SQLite ignores FK constraints unless this pragma is ON per connection — make
            // it explicit so opt-in `@fk` cascade/restrict/set_null actually enforce.
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            // The in-memory database *is* this one connection: never let the pool
            // retire it, or the data vanishes mid-test.
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_lazy_with(opts);
        Ok(SqliteBackend { pool })
    }

    /// A backend over a file database at `path` (created if absent). A file database
    /// persists on its own, so an ordinary bounded pool serves it.
    pub fn open(path: &str) -> Result<SqliteBackend, DbError> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_lazy_with(opts);
        Ok(SqliteBackend { pool })
    }

    /// Build the [`Backend`] over a caller's **existing** sqlx pool — the embed for an
    /// app that already owns a [`SqlitePool`] and wants the engine on it, not on a
    /// second pool. Cloning a pool is cheap (it is an `Arc` internally), so the app
    /// keeps using its handle while the engine uses this one.
    ///
    /// **The pool is used exactly as configured — their pool, their settings** (busy
    /// timeout, sizing, lifetimes). One caveat carries over from [`in_memory`]: a
    /// `:memory:` database is per-connection, so an in-memory pool must be pinned to a
    /// single never-expiring connection or each checkout sees a different empty
    /// database.
    ///
    /// [`in_memory`]: SqliteBackend::in_memory
    pub fn from_pool(pool: SqlitePool) -> SqliteBackend {
        SqliteBackend { pool }
    }

    /// Run setup SQL (e.g. `CREATE TABLE …; INSERT …;`) against the database — a
    /// multi-statement batch. A test seeds its schema + fixtures through this before
    /// dispatching requests.
    pub async fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(map_sqlite_err)
    }
}

#[async_trait]
impl Backend for SqliteBackend {
    async fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        // Single database: the shard key is ignored (SQLite has no shards).
        let conn = self.pool.acquire().await.map_err(map_sqlite_err)?;
        Ok(Box::new(SqliteDb { conn }))
    }

    async fn ping(&self) -> Result<(), DbError> {
        // A trivial round-trip validates the connection is usable right now.
        let mut db = SqliteDb {
            conn: self.pool.acquire().await.map_err(map_sqlite_err)?,
        };
        crate::run::fetch_all(db.fetch("SELECT 1", &[]))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::fetch_all;
    use serde_json::json;

    #[tokio::test]
    async fn round_trips_a_write_then_read() {
        // The seam works end to end against a real engine, no mock: create a table,
        // execute an INSERT with bound params, fetch it back shaped as JSON.
        let backend = SqliteBackend::in_memory().unwrap();
        let mut db = backend.checkout("").await.unwrap();
        db.execute("CREATE TABLE t (id TEXT, n INTEGER, ok BOOLEAN)", &[])
            .await
            .unwrap();
        db.execute(
            "INSERT INTO t (id, n, ok) VALUES (?, ?, ?)",
            &[
                SqlValue::Text("a".into()),
                SqlValue::Int(5),
                SqlValue::Bool(true),
            ],
        )
        .await
        .unwrap();
        let rows = fetch_all(db.fetch(
            "SELECT id, n, ok FROM t WHERE id = ?",
            &[SqlValue::Text("a".into())],
        ))
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], json!("a"));
        assert_eq!(rows[0]["n"], json!(5));
        assert_eq!(rows[0]["ok"], json!(1));
    }

    #[tokio::test]
    async fn shared_connection_persists_across_checkouts() {
        // An in-memory DB is per-connection; the backend recycles one connection, so a
        // write via one checkout is visible via the next (the integration-test property).
        let backend = SqliteBackend::in_memory().unwrap();
        backend
            .execute_batch("CREATE TABLE t (id TEXT)")
            .await
            .unwrap();
        backend
            .checkout("")
            .await
            .unwrap()
            .execute("INSERT INTO t (id) VALUES ('x')", &[])
            .await
            .unwrap();
        let mut db = backend.checkout("").await.unwrap();
        let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], json!("x"));
    }

    #[tokio::test]
    async fn ping_ok_on_a_live_db() {
        assert!(SqliteBackend::in_memory().unwrap().ping().await.is_ok());
    }

    #[tokio::test]
    async fn typed_text_riding_binds_round_trip_verbatim() {
        // The typed SqlValue variants bind their wire strings into TEXT storage and
        // read back exactly (decimals above all: every digit and trailing zero).
        let backend = SqliteBackend::in_memory().unwrap();
        let mut db = backend.checkout("").await.unwrap();
        db.execute(
            "CREATE TABLE t (id TEXT, at TEXT, day TEXT, total TEXT)",
            &[],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO t VALUES (?, ?, ?, ?)",
            &[
                SqlValue::Uuid("00000000-0000-4000-8000-0000000000a1".into()),
                SqlValue::Timestamp("2024-01-02 12:30:45.500000".into()),
                SqlValue::Date("2024-01-02".into()),
                SqlValue::Decimal("19.90".into()),
            ],
        )
        .await
        .unwrap();
        let rows = fetch_all(db.fetch("SELECT * FROM t", &[])).await.unwrap();
        assert_eq!(rows[0]["id"], json!("00000000-0000-4000-8000-0000000000a1"));
        assert_eq!(rows[0]["at"], json!("2024-01-02 12:30:45.500000"));
        assert_eq!(rows[0]["day"], json!("2024-01-02"));
        assert_eq!(rows[0]["total"], json!("19.90"));
    }

    #[tokio::test]
    async fn byo_pool_is_shared_with_the_caller() {
        // The BYO-pool embed: the app builds and owns the sqlx pool, uses it directly,
        // and hands a clone to the engine — one pool, two users. A write made through
        // the app's own sqlx query is visible through the Backend, and vice versa.
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_lazy_with(opts);
        sqlx::raw_sql("CREATE TABLE t (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (id) VALUES (?)")
            .bind("from-the-app")
            .execute(&pool)
            .await
            .unwrap();

        let backend = SqliteBackend::from_pool(pool.clone());
        let mut db = backend.checkout("").await.unwrap();
        let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], json!("from-the-app"));
        db.execute(
            "INSERT INTO t (id) VALUES (?)",
            &[SqlValue::Text("from-the-engine".into())],
        )
        .await
        .unwrap();
        drop(db); // return the single connection so the app's next query gets it

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2, "the engine's write landed on the app's pool");
    }

    #[tokio::test]
    async fn dropped_tx_rolls_back() {
        // The typestate guarantee on a real engine: a Tx dropped without commit leaves
        // no trace, and the connection remains usable for the next checkout.
        let backend = SqliteBackend::in_memory().unwrap();
        backend
            .execute_batch("CREATE TABLE t (id TEXT)")
            .await
            .unwrap();

        let db = backend.checkout("").await.unwrap();
        let mut tx = db.begin().await.unwrap();
        tx.execute("INSERT INTO t (id) VALUES ('doomed')", &[])
            .await
            .unwrap();
        drop(tx); // no commit

        let mut db = backend.checkout("").await.unwrap();
        let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
        assert!(rows.is_empty(), "an uncommitted write must not survive");
    }

    #[tokio::test]
    async fn committed_tx_persists() {
        let backend = SqliteBackend::in_memory().unwrap();
        backend
            .execute_batch("CREATE TABLE t (id TEXT)")
            .await
            .unwrap();

        let db = backend.checkout("").await.unwrap();
        let mut tx = db.begin().await.unwrap();
        tx.execute("INSERT INTO t (id) VALUES ('kept')", &[])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut db = backend.checkout("").await.unwrap();
        let rows = fetch_all(db.fetch("SELECT id FROM t", &[])).await.unwrap();
        assert_eq!(rows.len(), 1);
    }
}
