//! The concrete MariaDB driver + shard router (feature `mariadb`).
//!
//! This is the production [`Db`] behind the seam the mock stands in for. Two layers:
//!
//! - [`MariaDb`] — a [`Db`] over one pooled connection. It runs the whole request on
//!   that single connection (a mutation's `tx` must see its own writes), converting
//!   [`SqlValue`] binds to the driver's `?` parameters and driver rows back to JSON.
//!
//! - [`ShardRouter`] — the scale-out seam. It owns one bounded connection pool per
//!   physical shard and routes each request to exactly one shard (single-shard, no
//!   scatter-gather: a `tx` is one shard, so no distributed transaction; a down shard
//!   fails only its own traffic). Routing goes through a large fixed space of logical
//!   shards (a stable FNV hash of the shard key), which a small `logical → physical`
//!   assignment maps to a pool — so adding a physical shard moves some logical shards
//!   without rehashing every key.
//!
//! The value conversions ([`to_mysql`]/[`from_mysql`]) are pure and unit-tested below.

use std::time::Duration;

use mysql::prelude::Queryable;
use mysql::{Opts, OptsBuilder, Params, Pool, PoolConstraints, PoolOpts, PooledConn, Value};

use crate::run::{Backend, Db, DbError, DbErrorKind, Row};
use crate::shard::{fnv1a_64, LOGICAL_SHARDS};
use crate::value::SqlValue;

/// MariaDB deadlock (1213, `ER_LOCK_DEADLOCK`) and lock-wait timeout (1205,
/// `ER_LOCK_WAIT_TIMEOUT`): the server rolled the transaction back for lock contention, so
/// the mutation path may retry it. Everything else is an opaque operational `503`.
fn map_mysql_err(e: mysql::Error) -> DbError {
    let kind = match &e {
        mysql::Error::MySqlError(se) if se.code == 1213 || se.code == 1205 => DbErrorKind::Deadlock,
        _ => DbErrorKind::Other,
    };
    DbError::of(kind, e.to_string())
}

// Re-exported so `based_runtime::driver::{PoolConfig, ShardId}` paths still resolve; the
// routing primitives live in the backend-agnostic `crate::shard` module.
pub use crate::shard::{PoolConfig, ShardId};

// ---------- value conversion (pure, unit-tested) ---------------------------

/// A bound [`SqlValue`] → the driver's parameter value. A `bool` binds as MySQL's
/// tinyint `0/1`; `json` is sent as its serialized text (MySQL parses it into the `JSON`
/// column).
pub(crate) fn to_mysql(v: &SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::NULL,
        SqlValue::Int(i) => Value::Int(*i),
        SqlValue::Float(f) => Value::Double(*f),
        SqlValue::Bool(b) => Value::Int(*b as i64),
        SqlValue::Text(s) => Value::Bytes(s.clone().into_bytes()),
        SqlValue::Json(j) => Value::Bytes(j.to_string().into_bytes()),
    }
}

/// A returned column value → JSON (the shape the wire response is built from). Numbers
/// map to JSON numbers; `Bytes` is decoded as UTF-8 text (text/uuid/json/timestamp all
/// ride the wire as strings), falling back to lowercase hex for genuinely binary columns
/// (e.g. a `BINARY(16)` uuid where native `UUID` is unavailable). Date/Time render as
/// their canonical SQL string.
///
/// A `JSON` column comes back as a JSON-encoded string, not a reconstructed object: the
/// runtime does not carry per-column types into row shaping.
pub(crate) fn from_mysql(v: Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::NULL => J::Null,
        Value::Int(i) => J::Number(i.into()),
        Value::UInt(u) => J::Number(u.into()),
        Value::Float(f) => serde_json::Number::from_f64(f as f64).map_or(J::Null, J::Number),
        Value::Double(d) => serde_json::Number::from_f64(d).map_or(J::Null, J::Number),
        Value::Bytes(b) => match String::from_utf8(b) {
            Ok(s) => J::String(s),
            Err(e) => J::String(hex(e.as_bytes())),
        },
        Value::Date(y, mo, d, h, mi, s, us) => {
            let base = format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}");
            J::String(if us == 0 {
                base
            } else {
                format!("{base}.{us:06}")
            })
        }
        Value::Time(neg, days, h, mi, s, us) => {
            let sign = if neg { "-" } else { "" };
            let hours = days * 24 + h as u32;
            let base = format!("{sign}{hours:02}:{mi:02}:{s:02}");
            J::String(if us == 0 {
                base
            } else {
                format!("{base}.{us:06}")
            })
        }
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
    conn: PooledConn,
}

impl MariaDb {
    /// Wrap an already-checked-out connection (the router hands these out).
    pub fn new(conn: PooledConn) -> MariaDb {
        MariaDb { conn }
    }

    fn positional(params: &[SqlValue]) -> Params {
        Params::Positional(params.iter().map(to_mysql).collect())
    }
}

impl Db for MariaDb {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>, DbError> {
        let result = self
            .conn
            .exec_iter(sql, Self::positional(params))
            .map_err(map_mysql_err)?;
        let mut rows = Vec::new();
        // Build each row from its column names (the SELECT aliases each projection to
        // its output name, so a row is already the response object).
        for row in result {
            let row = row.map_err(map_mysql_err)?;
            let cols = row.columns();
            let mut obj = serde_json::Map::with_capacity(cols.len());
            for (col, val) in cols.iter().zip(row.unwrap()) {
                obj.insert(col.name_str().into_owned(), from_mysql(val));
            }
            rows.push(obj);
        }
        Ok(rows)
    }

    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.conn
            .exec_drop(sql, Self::positional(params))
            .map_err(map_mysql_err)?;
        Ok(self.conn.affected_rows())
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.conn
            .query_drop("START TRANSACTION")
            .map_err(map_mysql_err)
    }
    fn commit(&mut self) -> Result<(), DbError> {
        self.conn.query_drop("COMMIT").map_err(map_mysql_err)
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn.query_drop("ROLLBACK").map_err(map_mysql_err)
    }
}

// ---------- the shard router ------------------------------------------------

/// Routes each request to exactly one physical shard's connection pool. Holds the
/// pools (cheap to clone — each is an `Arc` internally, shared across worker threads)
/// and the permanent `logical → physical` assignment.
pub struct ShardRouter {
    /// One bounded pool per physical shard.
    shards: Vec<Pool>,
    /// `logical shard → physical shard index`; length is always [`LOGICAL_SHARDS`].
    assign: Vec<ShardId>,
    /// Max wait for a free connection before a checkout fails fast as pool-exhausted.
    checkout_timeout: Duration,
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
        Ok(ShardRouter {
            shards,
            assign,
            checkout_timeout: pool.checkout_timeout,
        })
    }

    /// The common case: one physical shard (all logical shards map to it). The router
    /// is still the seam — splitting later is a config change, not a code change.
    pub fn single(url: &str, pool: PoolConfig) -> Result<ShardRouter, DbError> {
        ShardRouter::new(std::slice::from_ref(&url.to_string()), pool)
    }

    /// The physical shard a key routes to: a stable logical hash, then the assignment.
    pub fn shard_for(&self, key: &str) -> ShardId {
        let logical = (fnv1a_64(key.as_bytes()) % LOGICAL_SHARDS as u64) as usize;
        self.assign[logical]
    }

    /// Check out a connection to the shard a key routes to (single-shard dispatch).
    pub fn checkout(&self, key: &str) -> Result<MariaDb, DbError> {
        self.checkout_shard(self.shard_for(key))
    }

    /// Check out a connection to a specific physical shard. Waits at most the configured
    /// `checkout_timeout` for a free connection, then fails fast as pool-exhausted —
    /// a saturated pool becomes a retryable `503`, never a hung worker.
    pub fn checkout_shard(&self, shard: ShardId) -> Result<MariaDb, DbError> {
        let pool = self
            .shards
            .get(shard)
            .ok_or_else(|| DbError::new(format!("no shard {shard}")))?;
        let conn = pool.try_get_conn(self.checkout_timeout).map_err(|e| {
            DbError::of(
                DbErrorKind::PoolExhausted,
                format!(
                    "connection pool exhausted (waited {:?}): {e}",
                    self.checkout_timeout
                ),
            )
        })?;
        Ok(MariaDb::new(conn))
    }

    /// How many physical shards the router spans.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

/// The router is the MariaDB [`Backend`]: it checks out a pooled [`MariaDb`] for the
/// key's shard. The HTTP edge depends only on this trait, so a future Postgres / MySQL
/// / SQLite backend is a drop-in without touching `based serve`.
impl Backend for ShardRouter {
    fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(ShardRouter::checkout(self, shard_key)?))
    }

    /// Readiness = *every* physical shard's pool can hand out a connection. A single
    /// down shard means this instance can't serve that shard's traffic, so the whole
    /// instance reports not-ready and the load balancer drains it (a partial-outage
    /// instance is worse than one fewer healthy instance). Each probe runs the driver's
    /// lightweight `SELECT 1` so a stale pooled connection is caught, not just a checkout.
    fn ping(&self) -> Result<(), DbError> {
        for shard in 0..self.shard_count() {
            let mut db = self.checkout_shard(shard)?;
            // A trivial round-trip validates the connection end to end (the pool may hand
            // out a socket the server has since closed); `fetch` surfaces that as a DbError.
            db.fetch("SELECT 1", &[])?;
        }
        Ok(())
    }
}

/// Build one shard's bounded connection pool from a `mysql://…` URL. Each new connection
/// runs an `init` that sets the session `max_statement_time` — MariaDB's per-query
/// server-side timeout (seconds; unlike MySQL's SELECT-only `max_execution_time`, it caps
/// every statement), so a runaway query is aborted rather than hanging the connection.
fn build_pool(url: &str, cfg: PoolConfig) -> Result<Pool, DbError> {
    let opts = Opts::from_url(url).map_err(|e| DbError::new(format!("bad database url: {e}")))?;
    let constraints = PoolConstraints::new(cfg.min, cfg.max)
        .ok_or_else(|| DbError::new("pool min must be <= max"))?;
    let mut builder =
        OptsBuilder::from_opts(opts).pool_opts(PoolOpts::new().with_constraints(constraints));
    if cfg.statement_timeout > Duration::ZERO {
        // `max_statement_time` is a float number of seconds; sub-second is honoured.
        let secs = cfg.statement_timeout.as_secs_f64();
        builder = builder.init(vec![format!("SET SESSION max_statement_time = {secs}")]);
    }
    Pool::new(builder).map_err(|e| DbError::new(format!("connecting to shard: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sqlvalue_to_mysql_families() {
        assert_eq!(to_mysql(&SqlValue::Null), Value::NULL);
        assert_eq!(to_mysql(&SqlValue::Int(7)), Value::Int(7));
        assert_eq!(to_mysql(&SqlValue::Float(1.5)), Value::Double(1.5));
        // bool rides as tinyint 0/1.
        assert_eq!(to_mysql(&SqlValue::Bool(true)), Value::Int(1));
        assert_eq!(to_mysql(&SqlValue::Bool(false)), Value::Int(0));
        assert_eq!(
            to_mysql(&SqlValue::Text("o-1".into())),
            Value::Bytes(b"o-1".to_vec())
        );
        // json is sent as serialized text.
        assert_eq!(
            to_mysql(&SqlValue::Json(json!({ "a": 1 }))),
            Value::Bytes(br#"{"a":1}"#.to_vec())
        );
    }

    #[test]
    fn mysql_value_to_json() {
        use serde_json::Value as J;
        assert_eq!(from_mysql(Value::NULL), J::Null);
        assert_eq!(from_mysql(Value::Int(42)), json!(42));
        assert_eq!(from_mysql(Value::UInt(42)), json!(42));
        assert_eq!(from_mysql(Value::Double(2.5)), json!(2.5));
        // text/uuid ride back as strings.
        assert_eq!(from_mysql(Value::Bytes(b"paid".to_vec())), json!("paid"));
        // a genuinely binary (non-UTF-8) value falls back to hex, never a panic.
        assert_eq!(from_mysql(Value::Bytes(vec![0xff, 0x01])), json!("ff01"));
        // datetime renders canonical, with micros only when present.
        assert_eq!(
            from_mysql(Value::Date(2026, 7, 3, 12, 30, 0, 0)),
            json!("2026-07-03 12:30:00")
        );
        assert_eq!(
            from_mysql(Value::Date(2026, 7, 3, 12, 30, 0, 500)),
            json!("2026-07-03 12:30:00.000500")
        );
    }

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
}
