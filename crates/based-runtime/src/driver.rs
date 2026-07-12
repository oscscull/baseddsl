//! The concrete MariaDB driver + shard router (feature `mariadb`), over sqlx's MySql
//! driver.
//!
//! This is the production [`Db`]/[`Backend`] behind the seam the mock stands in for.
//! sqlx is strictly the executor/pool layer: the SQL arrives already lowered with
//! positional binds, run via `sqlx::query` + per-value binds — no macros, no query
//! builder. Two layers:
//!
//! - [`MariaDb`] — a [`Db`] over one pooled connection. It runs the whole request on
//!   that single connection (a mutation's `tx` must see its own writes), converting
//!   [`SqlValue`] binds to driver parameters and driver rows back to JSON.
//!   [`Db::begin`] consumes it into a [`Tx`] wrapping sqlx's own transaction guard, so
//!   drop-without-commit rolls back and an open tx never re-enters the pool.
//!
//! - [`ShardRouter`] — the scale-out seam. It owns one bounded connection pool per
//!   physical shard and routes each request to exactly one shard (single-shard, no
//!   scatter-gather: a `tx` is one shard, so no distributed transaction; a down shard
//!   fails only its own traffic). Routing goes through a large fixed space of logical
//!   shards (a stable FNV hash of the shard key), which a small `logical → physical`
//!   assignment maps to a pool — so adding a physical shard moves some logical shards
//!   without rehashing every key.
//!
//! Value notes: every text-riding family (text/uuid/timestamp/date/decimal/json) binds
//! as its wire string — MariaDB coerces strings into every column type. Result columns:
//! native `UUID`/`JSON` arrive wire-flagged with the binary charset (typed BINARY/BLOB),
//! so they decode as raw bytes → UTF-8; `DECIMAL` decodes through `BigDecimal`, whose
//! `to_plain_string` re-renders the exact wire string. `rows_affected` reports *matched*
//! rows (sqlx sets `CLIENT_FOUND_ROWS`); nothing in the engine branches on the count.

use async_trait::async_trait;
use futures_util::StreamExt;
use sqlx::mysql::{MySql, MySqlArguments, MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::pool::PoolConnection;
use sqlx::query::Query;
use sqlx::{Column, Row as SqlxRow, TypeInfo, ValueRef};

use crate::run::{Backend, Db, DbError, DbErrorKind, DbRead, Row, RowStream, Tx};
use crate::shard::{fnv1a_64, LOGICAL_SHARDS};
use crate::value::SqlValue;

// Re-exported so `based_runtime::driver::{PoolConfig, ShardId}` paths still resolve; the
// routing primitives live in the backend-agnostic `crate::shard` module.
pub use crate::shard::{PoolConfig, ShardId};

/// MariaDB deadlock (1213, `ER_LOCK_DEADLOCK`) and lock-wait timeout (1205,
/// `ER_LOCK_WAIT_TIMEOUT`): the server rolled the transaction back for lock contention, so
/// the mutation path may retry it. Everything else is an opaque operational `503`.
fn map_mysql_err(e: sqlx::Error) -> DbError {
    let kind = match e
        .as_database_error()
        .and_then(|d| d.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>())
    {
        Some(se) if se.number() == 1213 || se.number() == 1205 => DbErrorKind::Deadlock,
        _ => DbErrorKind::Other,
    };
    DbError::of(kind, e.to_string())
}

/// A pool checkout failure: the bounded `acquire_timeout` elapsing is pool exhaustion
/// (the fast-503 path); anything else (host down, auth) is an opaque operational fault.
fn map_acquire_err(e: sqlx::Error) -> DbError {
    match e {
        sqlx::Error::PoolTimedOut => DbError::of(
            DbErrorKind::PoolExhausted,
            format!("connection pool exhausted: {e}"),
        ),
        other => DbError::new(format!("checking out a connection: {other}")),
    }
}

// ---------- value conversion ------------------------------------------------

/// Bind every [`SqlValue`] onto a query, positionally. A `bool` binds as MySQL's
/// tinyint `0/1`; every text-riding family binds its wire string (MariaDB coerces
/// strings into uuid/datetime/date/decimal/json columns); `json` is serialized text.
fn bind_all<'q>(
    mut q: Query<'q, MySql, MySqlArguments>,
    params: &[SqlValue],
) -> Query<'q, MySql, MySqlArguments> {
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
/// by its wire type. Numbers map to JSON numbers (`BOOLEAN` is tinyint → `0/1`);
/// `DECIMAL` re-renders its exact wire string via `BigDecimal::to_plain_string` (the
/// wire is text, but sqlx only surfaces it through a decimal type); datetime/date
/// render canonical (micros only when present); everything string-ish — including the
/// binary-charset-flagged native `UUID`/`JSON` columns — decodes as raw bytes → UTF-8,
/// falling back to lowercase hex for genuinely binary values (never a panic).
fn row_to_json(row: &MySqlRow) -> Result<Row, DbError> {
    use serde_json::Value as J;
    let mut obj = serde_json::Map::with_capacity(row.columns().len());
    for (i, col) in row.columns().iter().enumerate() {
        let raw = row.try_get_raw(i).map_err(map_mysql_err)?;
        let val = if raw.is_null() {
            J::Null
        } else {
            decode_mysql(row, i, raw.type_info().name())
                .map_err(|e| DbError::new(format!("decoding column `{}`: {e}", col.name())))?
        };
        obj.insert(col.name().to_string(), val);
    }
    Ok(obj)
}

fn decode_mysql(row: &MySqlRow, i: usize, ty: &str) -> Result<serde_json::Value, sqlx::Error> {
    use serde_json::Value as J;
    Ok(match ty {
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" => {
            J::Number(row.try_get_unchecked::<i64, _>(i)?.into())
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "BIGINT UNSIGNED" => J::Number(row.try_get_unchecked::<u64, _>(i)?.into()),
        "FLOAT" => serde_json::Number::from_f64(row.try_get_unchecked::<f32, _>(i)? as f64)
            .map_or(J::Null, J::Number),
        "DOUBLE" => serde_json::Number::from_f64(row.try_get_unchecked::<f64, _>(i)?)
            .map_or(J::Null, J::Number),
        "DECIMAL" => J::String(
            row.try_get_unchecked::<sqlx::types::BigDecimal, _>(i)?
                .to_plain_string(),
        ),
        "DATETIME" | "TIMESTAMP" => J::String(mysql_datetime(
            row.try_get_unchecked::<chrono::NaiveDateTime, _>(i)?,
        )),
        "DATE" => J::String(
            row.try_get_unchecked::<chrono::NaiveDate, _>(i)?
                .format("%Y-%m-%d")
                .to_string(),
        ),
        "TIME" => J::String(mysql_time(
            row.try_get_unchecked::<chrono::NaiveTime, _>(i)?,
        )),
        // Everything string-ish, including the binary-charset-flagged UUID/JSON
        // columns sqlx types BINARY/BLOB: raw bytes → UTF-8 (the value is already the
        // canonical text), hex for genuinely binary data.
        _ => match String::from_utf8(row.try_get_unchecked::<Vec<u8>, _>(i)?) {
            Ok(s) => J::String(s),
            Err(e) => J::String(hex(e.as_bytes())),
        },
    })
}

/// Canonical datetime string: seconds base, micros appended only when nonzero.
fn mysql_datetime(dt: chrono::NaiveDateTime) -> String {
    use chrono::Timelike;
    let micros = dt.nanosecond() / 1_000;
    let base = dt.format("%Y-%m-%d %H:%M:%S").to_string();
    if micros == 0 {
        base
    } else {
        format!("{base}.{micros:06}")
    }
}

/// Canonical time string, same micros convention as datetimes.
fn mysql_time(t: chrono::NaiveTime) -> String {
    use chrono::Timelike;
    let micros = t.nanosecond() / 1_000;
    let base = t.format("%H:%M:%S").to_string();
    if micros == 0 {
        base
    } else {
        format!("{base}.{micros:06}")
    }
}

/// Lowercase hex of a byte slice (for a non-UTF-8 binary column value).
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

// ---------- the concrete Db ------------------------------------------------

/// One pooled MariaDB connection, running one request. Checked out of a shard's pool
/// for the request's duration and returned on drop (the pool recycles it).
pub struct MariaDb {
    conn: PoolConnection<MySql>,
}

impl MariaDb {
    /// Wrap an already-checked-out connection (the router hands these out).
    pub fn new(conn: PoolConnection<MySql>) -> MariaDb {
        MariaDb { conn }
    }
}

#[async_trait]
impl DbRead for MariaDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let q = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params);
        Box::pin(
            q.fetch(&mut *self.conn)
                .map(|r| r.map_err(map_mysql_err).and_then(|row| row_to_json(&row))),
        )
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params)
            .execute(&mut *self.conn)
            .await
            .map(|d| d.rows_affected())
            .map_err(map_mysql_err)
    }
}

#[async_trait]
impl Db for MariaDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        // sqlx's transaction guard owns the pooled connection: commit consumes it,
        // drop without commit rolls back before the connection can be pooled again.
        let tx = sqlx::Transaction::begin(self.conn, None)
            .await
            .map_err(map_mysql_err)?;
        Ok(Box::new(MariaTx { tx }))
    }
}

/// An open MariaDB transaction (sqlx's guard over the same pooled connection).
struct MariaTx {
    tx: sqlx::Transaction<'static, MySql>,
}

#[async_trait]
impl DbRead for MariaTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let q = bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params);
        Box::pin(
            q.fetch(&mut *self.tx)
                .map(|r| r.map_err(map_mysql_err).and_then(|row| row_to_json(&row))),
        )
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params)
            .execute(&mut *self.tx)
            .await
            .map(|d| d.rows_affected())
            .map_err(map_mysql_err)
    }
}

#[async_trait]
impl Tx for MariaTx {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        self.tx.commit().await.map_err(map_mysql_err)
    }
}

// ---------- the shard router ------------------------------------------------

/// Routes each request to exactly one physical shard's connection pool. Holds the
/// pools (cheap to clone — each is an `Arc` internally) and the permanent
/// `logical → physical` assignment.
pub struct ShardRouter {
    /// One bounded pool per physical shard.
    shards: Vec<MySqlPool>,
    /// `logical shard → physical shard index`; length is always [`LOGICAL_SHARDS`].
    assign: Vec<ShardId>,
}

impl ShardRouter {
    /// Build a router over `urls` (one MariaDB per physical shard), each with a bounded
    /// pool, distributing the logical-shard space as evenly as possible across them.
    /// Adding a shard later re-runs this with the new URL list; only the logical shards
    /// that move need their data migrated — existing keys keep hashing the same.
    pub fn new(urls: &[String], pool: PoolConfig) -> Result<ShardRouter, DbError> {
        if urls.is_empty() {
            return Err(DbError::new("shard router needs at least one database url"));
        }
        let shards = urls
            .iter()
            .map(|u| build_pool(u, pool))
            .collect::<Result<Vec<_>, _>>()?;
        // Even round-robin split of the logical space across physical shards. This is
        // the default balance; a deployment can later hand-assign to move hot shards.
        let n = shards.len();
        let assign = (0..LOGICAL_SHARDS).map(|i| i % n).collect();
        Ok(ShardRouter { shards, assign })
    }

    /// The common case: one physical shard (all logical shards map to it). The router
    /// is still the seam — splitting later is a config change, not a code change.
    pub fn single(url: &str, pool: PoolConfig) -> Result<ShardRouter, DbError> {
        ShardRouter::new(std::slice::from_ref(&url.to_string()), pool)
    }

    /// Build the [`Backend`] over a caller's **existing** sqlx pool — the embed for an
    /// app that already owns a [`MySqlPool`] and wants the engine on it, not on a
    /// second pool. One physical shard; the shared codec/tx path is identical to a
    /// router built from a URL. Cloning a pool is cheap (it is an `Arc` internally),
    /// so the app keeps using its handle while the engine uses this one.
    ///
    /// **The pool is used exactly as configured — their pool, their settings.** The
    /// engine installs nothing on it: the session `max_statement_time` its own
    /// constructors apply rides `after_connect`, which only a pool's builder can set —
    /// and silently reconfiguring sessions the app's own queries share would be wrong
    /// anyway. Sizing, `acquire_timeout`, and connect hooks are all the caller's. A
    /// saturated pool still fails fast as
    /// [`PoolExhausted`](crate::run::DbErrorKind::PoolExhausted) when its own
    /// `acquire_timeout` elapses (sqlx's default is 30s), and deadlock-retry works
    /// unchanged (each attempt is a fresh checkout).
    pub fn from_pool(pool: MySqlPool) -> ShardRouter {
        ShardRouter {
            shards: vec![pool],
            assign: vec![0; LOGICAL_SHARDS],
        }
    }

    /// The physical shard a key routes to: a stable logical hash, then the assignment.
    pub fn shard_for(&self, key: &str) -> ShardId {
        let logical = (fnv1a_64(key.as_bytes()) % LOGICAL_SHARDS as u64) as usize;
        self.assign[logical]
    }

    /// Check out a connection to the shard a key routes to (single-shard dispatch).
    pub async fn checkout(&self, key: &str) -> Result<MariaDb, DbError> {
        self.checkout_shard(self.shard_for(key)).await
    }

    /// Check out a connection to a specific physical shard. Waits at most the pool's
    /// configured `acquire_timeout` for a free connection, then fails fast as
    /// pool-exhausted — a saturated pool becomes a retryable `503`, never a hung task.
    pub async fn checkout_shard(&self, shard: ShardId) -> Result<MariaDb, DbError> {
        let pool = self
            .shards
            .get(shard)
            .ok_or_else(|| DbError::new(format!("no shard {shard}")))?;
        let conn = pool.acquire().await.map_err(map_acquire_err)?;
        Ok(MariaDb::new(conn))
    }

    /// How many physical shards the router spans.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

/// The router is the MariaDB [`Backend`]: it checks out a pooled [`MariaDb`] for the
/// key's shard. The edges depend only on this trait, so another dialect's backend is a
/// drop-in without touching `based serve`.
#[async_trait]
impl Backend for ShardRouter {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(ShardRouter::checkout(self, shard_key).await?))
    }

    /// Readiness = *every* physical shard's pool can hand out a connection. A single
    /// down shard means this instance can't serve that shard's traffic, so the whole
    /// instance reports not-ready and the load balancer drains it (a partial-outage
    /// instance is worse than one fewer healthy instance). Each probe runs a
    /// lightweight `SELECT 1` so a stale pooled connection is caught, not just a checkout.
    async fn ping(&self) -> Result<(), DbError> {
        for shard in 0..self.shard_count() {
            let mut db = self.checkout_shard(shard).await?;
            crate::run::fetch_all(db.fetch("SELECT 1", &[])).await?;
        }
        Ok(())
    }
}

/// Build one shard's bounded connection pool from a `mysql://…` URL. Each new connection
/// runs an `after_connect` hook that sets the session `max_statement_time` — MariaDB's
/// per-query server-side timeout (seconds; unlike MySQL's SELECT-only
/// `max_execution_time`, it caps every statement), so a runaway query is aborted rather
/// than hanging the connection. The pool's `acquire_timeout` is the checkout wait
/// (pool-exhaustion → fast `503`).
fn build_pool(url: &str, cfg: PoolConfig) -> Result<MySqlPool, DbError> {
    let mut opts = MySqlPoolOptions::new()
        .min_connections(cfg.min as u32)
        .max_connections(cfg.max as u32)
        .acquire_timeout(cfg.checkout_timeout);
    if cfg.statement_timeout > std::time::Duration::ZERO {
        // `max_statement_time` is a float number of seconds; sub-second is honoured.
        let stmt = format!(
            "SET SESSION max_statement_time = {}",
            cfg.statement_timeout.as_secs_f64()
        );
        opts = opts.after_connect(move |conn, _meta| {
            let stmt = stmt.clone();
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(stmt.clone()))
                    .execute(conn)
                    .await
                    .map(|_| ())
            })
        });
    }
    opts.connect_lazy(url)
        .map_err(|e| DbError::new(format!("bad database url: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::LOGICAL_SHARDS;

    #[test]
    fn shard_routing_is_stable_and_in_range() {
        // Stable: the same key always lands on the same logical shard (regression
        // guard on the pinned FNV constants — a routing change would strand data).
        assert_eq!(fnv1a_64(b"org-1") % LOGICAL_SHARDS as u64, {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for &b in b"org-1" {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h % LOGICAL_SHARDS as u64
        });
    }

    #[test]
    fn datetime_renders_canonical() {
        use chrono::NaiveDate;
        let base = NaiveDate::from_ymd_opt(2026, 7, 3)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        assert_eq!(mysql_datetime(base), "2026-07-03 12:30:00");
        let with_micros = base + chrono::Duration::microseconds(500);
        assert_eq!(mysql_datetime(with_micros), "2026-07-03 12:30:00.000500");
    }

    #[test]
    fn hex_of_binary_bytes() {
        assert_eq!(hex(&[0xff, 0x01]), "ff01");
    }
}
