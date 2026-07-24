//! The concrete Postgres driver + shard router (feature `postgres`), over sqlx's
//! Postgres driver.
//!
//! The Postgres twin of the MariaDB driver ([`crate::driver`]): the production
//! [`Db`]/[`Backend`] that runs the verbatim Postgres-lowered SQL (`$n`-bound) against a
//! real server. sqlx is strictly the executor/pool layer — `sqlx::query` + per-value
//! binds, no macros, no query builder. Structure mirrors the MariaDB driver:
//! [`PostgresDb`] (one pooled connection), a typestate transaction guard, and
//! [`PgRouter`] (one bounded pool per physical shard, the same stable FNV routing).
//!
//! **The value-mapping subtlety (Postgres-specific).** sqlx transmits every parameter
//! in *binary* format under a client-declared OID, so a wire-text bind the server
//! coerces (the old sync driver's trick) is impossible: a `text`-declared string is a
//! hard type error against a `uuid`/`timestamptz`/`jsonb`/`numeric` column. So this
//! driver binds **native types**, parsed from the typed [`SqlValue`] variants' wire
//! strings — uuid, chrono timestamps/dates, `serde_json::Value`, `BigDecimal` (the
//! mandated decimal bind: value-exact at full precision; the column's typmod rescales
//! storage). A [`SqlValue::Null`] binds as an *unknown*-typed NULL, which the server
//! resolves to the target column's type like a bare NULL literal.
//!
//! **Results are binary too** — and for `numeric` neither sqlx decimal feature's decode
//! yields the exact wire string, so the hand-rolled [`pg_numeric`] decoder survives,
//! fed sqlx's untouched raw bytes. The uuid/timestamp/date/jsonb binary layouts get the
//! same treatment (pure decoders, unit-tested below), so every value rides back as the
//! same canonical string a literal would produce.

use std::str::FromStr;

use async_trait::async_trait;
use futures_util::StreamExt;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgArguments, PgPool, PgPoolOptions, PgRow, Postgres};
use sqlx::query::Query;
use sqlx::{Column, Row as SqlxRow, TypeInfo, ValueRef};

use crate::run::{Backend, Db, DbError, DbErrorKind, DbRead, Row, RowStream, Tx};
use crate::shard::{fnv1a_64, PoolConfig, ShardId, LOGICAL_SHARDS};
use crate::value::SqlValue;

/// Postgres deadlock (`40P01`, `deadlock_detected`) and serialization failure (`40001`): the
/// server rolled the transaction back for a concurrency conflict, so the mutation path may
/// retry it. A `statement_timeout` cancel (`57014`) is not retried — re-running would just
/// time out again — so it stays [`Other`](DbErrorKind::Other) → an opaque `503`.
fn map_pg_err(e: sqlx::Error) -> DbError {
    let kind = match e
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
    {
        Some(c) if c == "40P01" || c == "40001" => DbErrorKind::Deadlock,
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

// ---------- binding ----------------------------------------------------------

/// An untyped SQL NULL: declared with the `unknown` OID so the server resolves the
/// parameter to the target column's type — exactly how a bare `NULL` literal behaves.
/// (A typed `Option::<T>::None` would declare `T`'s OID and mismatch other columns.)
struct PgNull;

impl sqlx::Type<Postgres> for PgNull {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(705)) // unknown
    }
}

impl sqlx::Encode<'_, Postgres> for PgNull {
    fn encode_by_ref(
        &self,
        _buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        Ok(sqlx::encode::IsNull::Yes)
    }
}

/// Bind every [`SqlValue`] onto a query, positionally, as its native Postgres type.
/// The typed text-riding variants parse here — and only here; the rest of the runtime
/// carries wire strings. A value that does not parse is an operational error (the same
/// class the server's own rejection of a bad literal used to be).
fn bind_all<'q>(
    mut q: Query<'q, Postgres, PgArguments>,
    params: &[SqlValue],
) -> Result<Query<'q, Postgres, PgArguments>, DbError> {
    for v in params {
        q = match v {
            SqlValue::Null => q.bind(PgNull),
            SqlValue::Int(i) => q.bind(*i),
            SqlValue::Float(f) => q.bind(*f),
            SqlValue::Bool(b) => q.bind(*b),
            SqlValue::Text(s) => q.bind(s.clone()),
            SqlValue::Uuid(s) => q.bind(
                sqlx::types::Uuid::parse_str(s)
                    .map_err(|e| DbError::new(format!("invalid uuid `{s}`: {e}")))?,
            ),
            SqlValue::Timestamp(s) => q.bind(
                parse_timestamp(s)
                    .ok_or_else(|| DbError::new(format!("invalid timestamp `{s}`")))?,
            ),
            SqlValue::Date(s) => q.bind(
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .map_err(|e| DbError::new(format!("invalid date `{s}`: {e}")))?,
            ),
            SqlValue::Decimal(s) => q.bind(
                sqlx::types::BigDecimal::from_str(s)
                    .map_err(|e| DbError::new(format!("invalid decimal `{s}`: {e}")))?,
            ),
            SqlValue::Json(j) => q.bind(j.clone()),
        };
    }
    Ok(q)
}

/// Parse a wire timestamp into UTC. Accepts the engine's own canonical form
/// (`2024-01-02 12:30:45.500000+00`), RFC 3339, and a naive datetime (space or `T`
/// separated, taken as UTC — matching how the server reads an offset-less literal into
/// `timestamptz` under the UTC timezone the engine assumes).
fn parse_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%dT%H:%M:%S%.f%#z"] {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc());
        }
    }
    None
}

// ---------- decoding ----------------------------------------------------------

/// One result row → JSON (the shape the wire response is built from), each column
/// decoded from its raw binary bytes by its Postgres type. Numbers map to JSON
/// numbers; every string-family type rides back as its canonical string via the pure
/// decoders below; a genuinely unknown binary type falls back to lowercase hex (never
/// a panic).
fn row_to_json(row: &PgRow) -> Result<Row, DbError> {
    use serde_json::Value as J;
    let mut obj = serde_json::Map::with_capacity(row.columns().len());
    for (i, col) in row.columns().iter().enumerate() {
        let raw = row.try_get_raw(i).map_err(map_pg_err)?;
        let val = if raw.is_null() {
            J::Null
        } else {
            let bytes = raw
                .as_bytes()
                .map_err(|e| DbError::new(format!("reading column `{}`: {e}", col.name())))?;
            match raw.format() {
                sqlx::postgres::PgValueFormat::Binary => {
                    decode_pg_binary(col.type_info().name(), bytes)
                }
                // Text format (a simple-protocol result): the bytes are already the
                // canonical text of any type.
                sqlx::postgres::PgValueFormat::Text => decode_pg_text(
                    col.type_info().name(),
                    std::str::from_utf8(bytes).unwrap_or_default(),
                ),
            }
        };
        obj.insert(col.name().to_string(), val);
    }
    Ok(obj)
}

/// Decode one binary-format Postgres value by its type name.
fn decode_pg_binary(ty: &str, b: &[u8]) -> serde_json::Value {
    use serde_json::Value as J;
    match ty {
        "BOOL" => J::Bool(b.first().is_some_and(|v| *v != 0)),
        "INT2" => read_i16(b).map_or(J::Null, |n| J::Number(i64::from(n).into())),
        "INT4" => read_i32(b).map_or(J::Null, |n| J::Number(i64::from(n).into())),
        "INT8" => read_i64(b).map_or(J::Null, |n| J::Number(n.into())),
        "FLOAT4" => read_i32(b)
            .and_then(|n| serde_json::Number::from_f64(f64::from(f32::from_bits(n as u32))))
            .map_or(J::Null, J::Number),
        "FLOAT8" => read_i64(b)
            .and_then(|n| serde_json::Number::from_f64(f64::from_bits(n as u64)))
            .map_or(J::Null, J::Number),
        "UUID" => pg_uuid(b),
        "TIMESTAMPTZ" | "TIMESTAMP" => pg_timestamp(b),
        "DATE" => pg_date(b),
        "JSONB" => pg_jsonb(b),
        // `numeric`/`decimal` carries a packed base-10000 digit array, not its text — so a
        // decimal returns as its exact string (a JSON string, the wire form), losing no digit.
        "NUMERIC" => pg_numeric(b),
        // Everything else — text/varchar, `json`, etc. — has a binary form that *is* its
        // UTF-8 text; a genuinely binary value falls back to hex.
        _ => match std::str::from_utf8(b) {
            Ok(s) => serde_json::Value::String(s.to_string()),
            Err(_) => serde_json::Value::String(hex(b)),
        },
    }
}

/// Decode one text-format Postgres value: the text is already canonical; numbers and
/// bools re-typed into JSON.
fn decode_pg_text(ty: &str, s: &str) -> serde_json::Value {
    use serde_json::Value as J;
    match ty {
        "BOOL" => J::Bool(s == "t"),
        "INT2" | "INT4" | "INT8" => s.parse::<i64>().map_or(J::Null, |n| J::Number(n.into())),
        "FLOAT4" | "FLOAT8" => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(J::Null, J::Number),
        _ => J::String(s.to_string()),
    }
}

/// Postgres's binary temporal epoch: 2000-01-01 is 10957 days after the Unix epoch, the
/// offset that turns "days since 2000" into the days-since-1970 the civil conversion uses.
const PG_EPOCH_DAYS_FROM_UNIX: i64 = 10957;
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// A binary `uuid` (16 raw bytes) → the canonical hyphenated `8-4-4-4-12` string. A hex fall
/// back would drop the hyphens — a technically-parseable but non-canonical form; this emits
/// the real thing.
fn pg_uuid(b: &[u8]) -> serde_json::Value {
    if b.len() != 16 {
        return serde_json::Value::String(hex(b));
    }
    let h = hex(b);
    serde_json::Value::String(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32],
    ))
}

/// A binary `timestamptz`/`timestamp` (an i64 of microseconds since 2000-01-01 UTC) → an
/// ISO string `YYYY-MM-DD HH:MM:SS[.ffffff]+00` Postgres parses back to the same instant
/// (so a keyset cursor's timestamp basis compares exactly equal on the next page).
fn pg_timestamp(b: &[u8]) -> serde_json::Value {
    let Some(micros) = read_i64(b) else {
        return serde_json::Value::String(hex(b));
    };
    let days = micros.div_euclid(MICROS_PER_DAY);
    let tod = micros.rem_euclid(MICROS_PER_DAY); // microseconds into the day, always ≥ 0
    let (y, m, d) = civil_from_days(days + PG_EPOCH_DAYS_FROM_UNIX);
    let (secs, frac) = (tod / 1_000_000, tod % 1_000_000);
    let (hh, mm, ss) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let mut s = format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}");
    if frac != 0 {
        s.push_str(&format!(".{frac:06}"));
    }
    s.push_str("+00");
    serde_json::Value::String(s)
}

/// A binary `date` (an i32 of days since 2000-01-01) → an ISO `YYYY-MM-DD` string.
fn pg_date(b: &[u8]) -> serde_json::Value {
    let Some(days) = read_i32(b) else {
        return serde_json::Value::String(hex(b));
    };
    let (y, m, d) = civil_from_days(i64::from(days) + PG_EPOCH_DAYS_FROM_UNIX);
    serde_json::Value::String(format!("{y:04}-{m:02}-{d:02}"))
}

/// A binary `jsonb` (a leading version byte — always `1` today — then the JSON text) → the
/// JSON text (the wire carries JSON as a string). Strips the version byte a raw read would
/// otherwise prepend.
fn pg_jsonb(b: &[u8]) -> serde_json::Value {
    match b.split_first() {
        Some((1, rest)) => match std::str::from_utf8(rest) {
            Ok(s) => serde_json::Value::String(s.to_string()),
            Err(_) => serde_json::Value::String(hex(b)),
        },
        _ => match std::str::from_utf8(b) {
            Ok(s) => serde_json::Value::String(s.to_string()),
            Err(_) => serde_json::Value::String(hex(b)),
        },
    }
}

/// A binary `numeric`/`decimal` → its canonical decimal string. The wire layout is four
/// `int16` header fields (digit count, weight, sign, display scale) then that many base-10000
/// digits. Reconstructed exactly (no float), so the value round-trips to the same string the
/// column stores. `NaN` renders `"NaN"`; a malformed buffer falls back to hex.
fn pg_numeric(b: &[u8]) -> serde_json::Value {
    use serde_json::Value as J;
    if b.len() < 8 {
        return J::String(hex(b));
    }
    let rd = |o: usize| i16::from_be_bytes([b[o], b[o + 1]]);
    let ndigits = rd(0);
    let weight = i32::from(rd(2));
    let sign = rd(4) as u16;
    let dscale = rd(6).max(0) as usize;
    if sign == 0xC000 {
        return J::String("NaN".to_string());
    }
    if ndigits < 0 || b.len() < 8 + ndigits as usize * 2 {
        return J::String(hex(b));
    }
    let ndigits = ndigits as usize;

    // Concatenate the base-10000 groups into one decimal-digit run (each group is 4 digits,
    // the most significant group keeps its natural width). The decimal point then sits
    // `4 * (ndigits - 1 - weight)` digits from the right of that run.
    let mut run = String::with_capacity(ndigits * 4);
    for i in 0..ndigits {
        run.push_str(&format!("{:04}", rd(8 + i * 2)));
    }
    let point_from_right = 4 * (ndigits as i32 - 1 - weight);

    let (int_part, mut frac_part) = if point_from_right <= 0 {
        let mut whole = run;
        whole.push_str(&"0".repeat((-point_from_right) as usize));
        (whole, String::new())
    } else {
        let pfr = point_from_right as usize;
        let padded = if run.len() < pfr {
            format!("{}{}", "0".repeat(pfr - run.len()), run)
        } else {
            run
        };
        let split = padded.len() - pfr;
        (padded[..split].to_string(), padded[split..].to_string())
    };

    let int_trimmed = int_part.trim_start_matches('0');
    let int_final = if int_trimmed.is_empty() {
        "0"
    } else {
        int_trimmed
    };
    if frac_part.len() < dscale {
        frac_part.push_str(&"0".repeat(dscale - frac_part.len()));
    } else if frac_part.len() > dscale {
        frac_part.truncate(dscale);
    }

    let is_zero = int_final == "0" && frac_part.chars().all(|c| c == '0');
    let mut out = String::new();
    if sign == 0x4000 && !is_zero {
        out.push('-');
    }
    out.push_str(int_final);
    if dscale > 0 {
        out.push('.');
        out.push_str(&frac_part);
    }
    J::String(out)
}

fn read_i16(b: &[u8]) -> Option<i16> {
    b.try_into().ok().map(i16::from_be_bytes)
}

fn read_i64(b: &[u8]) -> Option<i64> {
    b.try_into().ok().map(i64::from_be_bytes)
}

fn read_i32(b: &[u8]) -> Option<i32> {
    b.try_into().ok().map(i32::from_be_bytes)
}

/// Civil date (year, month, day) from a day count since the Unix epoch (1970-01-01) — Howard
/// Hinnant's branchless `civil_from_days` algorithm, valid across the whole proleptic
/// Gregorian range with no date library. Month is `1..=12`, day `1..=31`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // day of era, [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // year of era, [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year (Mar-based), [0, 365]
    let mp = (5 * doy + 2) / 153; // Mar-based month, [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // day, [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // Jan-based month, [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Lowercase hex of a byte slice (for a non-UTF-8 binary column value).
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap());
    }
    s
}

// ---------- the concrete Db ------------------------------------------------

/// One pooled Postgres connection, running one request. Checked out of a shard's pool for
/// the request's duration and returned on drop (the pool recycles it).
pub struct PostgresDb {
    conn: PoolConnection<Postgres>,
}

impl PostgresDb {
    /// Wrap an already-checked-out connection (the router hands these out).
    pub fn new(conn: PoolConnection<Postgres>) -> Self {
        Self { conn }
    }
}

/// A fetch whose binds failed to parse: a single-item error stream (the one read path
/// still reports it as a stream item).
fn err_stream<'a>(e: DbError) -> RowStream<'a> {
    Box::pin(futures_util::stream::iter([Err(e)]))
}

#[async_trait]
impl DbRead for PostgresDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let q = match bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params) {
            Ok(q) => q,
            Err(e) => return err_stream(e),
        };
        Box::pin(
            q.fetch(&mut *self.conn)
                .map(|r| r.map_err(map_pg_err).and_then(|row| row_to_json(&row))),
        )
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params)?
            .execute(&mut *self.conn)
            .await
            .map(|d| d.rows_affected())
            .map_err(map_pg_err)
    }
}

#[async_trait]
impl Db for PostgresDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        let tx = sqlx::Transaction::begin(self.conn, None)
            .await
            .map_err(map_pg_err)?;
        Ok(Box::new(PgTx { tx }))
    }
}

/// An open Postgres transaction (sqlx's guard over the same pooled connection).
struct PgTx {
    tx: sqlx::Transaction<'static, Postgres>,
}

#[async_trait]
impl DbRead for PgTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let q = match bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params) {
            Ok(q) => q,
            Err(e) => return err_stream(e),
        };
        Box::pin(
            q.fetch(&mut *self.tx)
                .map(|r| r.map_err(map_pg_err).and_then(|row| row_to_json(&row))),
        )
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        bind_all(sqlx::query(sqlx::AssertSqlSafe(sql)), params)?
            .execute(&mut *self.tx)
            .await
            .map(|d| d.rows_affected())
            .map_err(map_pg_err)
    }
}

#[async_trait]
impl Tx for PgTx {
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        self.tx.commit().await.map_err(map_pg_err)
    }
}

// ---------- the shard router ------------------------------------------------

/// Routes each request to exactly one physical Postgres shard's connection pool — the
/// [`crate::driver::ShardRouter`] twin. Holds one bounded pool per shard and the
/// permanent `logical → physical` assignment (the same stable FNV routing the MariaDB
/// router uses, so a key hashes identically regardless of the backend dialect).
pub struct PgRouter {
    shards: Vec<PgPool>,
    /// `logical shard → physical shard index`; length is always [`LOGICAL_SHARDS`].
    assign: Vec<ShardId>,
}

impl PgRouter {
    /// Build a router over `urls` (one Postgres per physical shard), each with a bounded
    /// pool. Adding a shard later re-runs this with the new URL list; only the logical
    /// shards that move need migrating — existing keys keep hashing the same.
    pub fn new(urls: &[String], pool: PoolConfig) -> Result<Self, DbError> {
        if urls.is_empty() {
            return Err(DbError::new("shard router needs at least one database url"));
        }
        let shards = urls
            .iter()
            .map(|u| build_pool(u, pool))
            .collect::<Result<Vec<_>, _>>()?;
        let n = shards.len();
        let assign = (0..LOGICAL_SHARDS).map(|i| i % n).collect();
        Ok(Self { shards, assign })
    }

    /// The common case: one physical shard (all logical shards map to it). The router is
    /// still the seam — splitting later is a config change, not a code change.
    pub fn single(url: &str, pool: PoolConfig) -> Result<Self, DbError> {
        Self::new(std::slice::from_ref(&url.to_string()), pool)
    }

    /// Build the [`Backend`] over a caller's **existing** sqlx pool — the embed for an
    /// app that already owns a [`PgPool`] and wants the engine on it, not on a second
    /// pool. One physical shard; the shared codec/tx path is identical to a router
    /// built from a URL. Cloning a pool is cheap (it is an `Arc` internally), so the
    /// app keeps using its handle while the engine uses this one.
    ///
    /// **The pool is used exactly as configured — their pool, their settings.** The
    /// engine installs nothing on it: the session `statement_timeout` its own
    /// constructors apply rides `after_connect`, which only a pool's builder can set —
    /// and silently reconfiguring sessions the app's own queries share would be wrong
    /// anyway. Sizing, `acquire_timeout`, and connect hooks are all the caller's. A
    /// saturated pool still fails fast as
    /// [`PoolExhausted`](crate::run::DbErrorKind::PoolExhausted) when its own
    /// `acquire_timeout` elapses (sqlx's default is 30s), and deadlock-retry works
    /// unchanged (each attempt is a fresh checkout).
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
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
    pub async fn checkout(&self, key: &str) -> Result<PostgresDb, DbError> {
        self.checkout_shard(self.shard_for(key)).await
    }

    /// Check out a connection to a specific physical shard. Waits at most the pool's
    /// configured `acquire_timeout` for a free connection, then fails fast as
    /// pool-exhausted — a saturated pool becomes a retryable `503`, never a hung task.
    pub async fn checkout_shard(&self, shard: ShardId) -> Result<PostgresDb, DbError> {
        let pool = self
            .shards
            .get(shard)
            .ok_or_else(|| DbError::new(format!("no shard {shard}")))?;
        let conn = pool.acquire().await.map_err(map_acquire_err)?;
        Ok(PostgresDb::new(conn))
    }

    /// How many physical shards the router spans.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

/// The router is the Postgres [`Backend`]: it checks out a pooled [`PostgresDb`] for the
/// key's shard — a drop-in beside the MariaDB `ShardRouter` with no change to `based serve`
/// (the `Db` seam is dialect-agnostic; only the `Compiled.dialect` must match the backend,
/// a deployment invariant).
#[async_trait]
impl Backend for PgRouter {
    async fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(Self::checkout(self, shard_key).await?))
    }

    /// Readiness = every physical shard's pool can hand out a connection that answers
    /// `SELECT 1` (a stale pooled socket is caught, not just a checkout) — the same
    /// all-shards-ready rule as the MariaDB router.
    async fn ping(&self) -> Result<(), DbError> {
        for shard in 0..self.shard_count() {
            let mut db = self.checkout_shard(shard).await?;
            crate::run::fetch_all(db.fetch("SELECT 1", &[])).await?;
        }
        Ok(())
    }
}

/// Build one shard's bounded connection pool from a `postgres://…` URL. The pool's
/// `acquire_timeout` is the checkout wait (pool-exhaustion → fast `503`), and each new
/// connection's `after_connect` hook sets the server-side `statement_timeout` so every
/// statement on a pooled connection is capped — a runaway query is cancelled (`57014`)
/// rather than hanging.
fn build_pool(url: &str, cfg: PoolConfig) -> Result<PgPool, DbError> {
    let mut opts = PgPoolOptions::new()
        .min_connections(cfg.min as u32)
        .max_connections(cfg.max as u32)
        .acquire_timeout(cfg.checkout_timeout);
    if cfg.statement_timeout > std::time::Duration::ZERO {
        let stmt = format!(
            "SET statement_timeout = {}",
            cfg.statement_timeout.as_millis()
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
    use serde_json::json;

    #[test]
    fn hex_of_binary_bytes() {
        assert_eq!(hex(&[0xff, 0x01]), "ff01");
    }

    #[test]
    fn binary_numeric_decodes_to_exact_string() {
        // Build a Postgres binary numeric: header (ndigits, weight, sign, dscale) then the
        // base-10000 digit groups, all i16 big-endian.
        fn enc(ndigits: i16, weight: i16, sign: u16, dscale: i16, digits: &[i16]) -> Vec<u8> {
            let mut b = Vec::new();
            for v in [ndigits, weight, sign as i16, dscale] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            for d in digits {
                b.extend_from_slice(&d.to_be_bytes());
            }
            b
        }
        // 9.99 — one integer group (9) + one fractional group (9900), scale 2.
        assert_eq!(pg_numeric(&enc(2, 0, 0, 2, &[9, 9900])), json!("9.99"));
        // 0.10 — the trailing zero is preserved (a float read would drop it).
        assert_eq!(pg_numeric(&enc(1, -1, 0, 2, &[1000])), json!("0.10"));
        // 12345678.90 — two integer groups + one fractional group.
        assert_eq!(
            pg_numeric(&enc(3, 1, 0, 2, &[1234, 5678, 9000])),
            json!("12345678.90")
        );
        // Negative, scale 1.
        assert_eq!(pg_numeric(&enc(2, 0, 0x4000, 1, &[5, 5000])), json!("-5.5"));
        // Zero with a display scale.
        assert_eq!(pg_numeric(&enc(0, 0, 0, 2, &[])), json!("0.00"));
    }

    /// The binary decoders round-trip a Postgres binary field into its canonical string —
    /// the read path for a `uuid`/`timestamptz`/`date`/`jsonb` result column (sqlx returns
    /// results in *binary* format). Proven live in `tests/postgres_integration.rs`; here
    /// the pure byte→string mapping is unit-covered.
    #[test]
    fn binary_uuid_decodes_to_canonical_string() {
        let bytes = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0xa1,
        ];
        assert_eq!(
            pg_uuid(&bytes),
            json!("00000000-0000-4000-8000-0000000000a1")
        );
        // A wrong length never panics — it falls back to hex.
        assert_eq!(pg_uuid(&[0x01, 0x02]), json!("0102"));
    }

    #[test]
    fn binary_timestamp_decodes_to_iso() {
        // The Postgres epoch itself (0 microseconds since 2000-01-01 00:00:00 UTC).
        assert_eq!(
            pg_timestamp(&0i64.to_be_bytes()),
            json!("2000-01-01 00:00:00+00")
        );
        // One day + 1s500000µs past the epoch → 2000-01-02 00:00:01.500000+00.
        let micros = MICROS_PER_DAY + 1_500_000;
        assert_eq!(
            pg_timestamp(&micros.to_be_bytes()),
            json!("2000-01-02 00:00:01.500000+00")
        );
    }

    #[test]
    fn binary_date_decodes_to_iso() {
        assert_eq!(pg_date(&0i32.to_be_bytes()), json!("2000-01-01"));
        assert_eq!(pg_date(&31i32.to_be_bytes()), json!("2000-02-01"));
    }

    #[test]
    fn binary_jsonb_strips_version_byte() {
        let mut b = vec![1u8];
        b.extend_from_slice(br#"{"a":1}"#);
        assert_eq!(pg_jsonb(&b), json!(r#"{"a":1}"#));
    }

    /// A leap-year boundary the civil conversion must get right (2000 is a leap year).
    #[test]
    fn civil_from_days_handles_leap_year() {
        // 2000-02-29 is day 59 since 2000-01-01.
        let (y, m, d) = civil_from_days(59 + PG_EPOCH_DAYS_FROM_UNIX);
        assert_eq!((y, m, d), (2000, 2, 29));
    }

    /// The typed bind path round-trips the engine's own canonical timestamp string —
    /// the exact form `pg_timestamp` emits — plus the common external forms.
    #[test]
    fn parse_timestamp_accepts_wire_forms() {
        for s in [
            "2024-01-02 12:30:45.500000+00",
            "2024-01-02 12:30:45+00",
            "2024-01-02T12:30:45.5Z",
            "2024-01-02T12:30:45+00:00",
            "2024-01-02 12:30:45",
            "2024-01-02T12:30:45.500000",
        ] {
            assert!(parse_timestamp(s).is_some(), "should parse: {s}");
        }
        assert!(parse_timestamp("not a time").is_none());
        // The canonical wire string parses back to the same instant it encodes.
        let dt = parse_timestamp("2024-01-02 12:30:45.500000+00").unwrap();
        assert_eq!(
            dt.timestamp_micros(),
            parse_timestamp("2024-01-02T12:30:45.5Z")
                .unwrap()
                .timestamp_micros()
        );
    }
}
