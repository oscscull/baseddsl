//! The concrete Postgres driver + shard router (feature `postgres`).
//!
//! This is the Postgres twin of the MariaDB driver ([`crate::driver`]): the production
//! [`Db`]/[`Backend`] that runs the verbatim Postgres-lowered SQL against a real server.
//! Two layers, exactly mirroring the MariaDB structure:
//!
//! - [`PostgresDb`] — a [`Db`] over one pooled connection. It runs the whole request on
//!   that single connection (a mutation's `tx` must see its own writes), converting
//!   [`SqlValue`] binds to Postgres parameters and returned columns back to JSON. It reuses
//!   the pure-Rust synchronous `postgres` crate (no async runtime).
//!
//! - [`PgRouter`] — the scale-out seam, the [`crate::driver::ShardRouter`] twin. One
//!   bounded `r2d2` connection pool per physical shard, single-shard dispatch by the same
//!   stable FNV logical-shard hash (no scatter-gather → a `tx` is one shard, no distributed
//!   transaction; add capacity without rehashing keys).
//!
//! **The value-mapping subtlety (Postgres-specific).** The runtime is dialect-neutral: a
//! `uuid`/`timestamptz`/`jsonb` value is carried as [`SqlValue::Text`] (a String — on the
//! wire these are all strings). Postgres, unlike MySQL/SQLite, infers each `$n` parameter's
//! type from the column it binds against and, in the extended protocol, refuses a
//! `text`-encoded Rust `String` for an inferred `uuid`/`jsonb` OID. So [`PgValue`] is a
//! `ToSql` newtype that (a) `accepts` those non-text OIDs and (b) encodes its bytes in text
//! format ([`Format::Text`]) — the server then applies its normal string-literal coercion
//! (the same path `'…'::uuid` takes). This keeps the runtime free of per-column Postgres
//! types while round-tripping every family. The mapping is pure and unit-tested below;
//! connecting/executing is proven by `tests/postgres_integration.rs` against a live server.

use std::time::Duration;

use bytes::BytesMut;
use postgres::error::SqlState;
use postgres::types::{to_sql_checked, Format, IsNull, ToSql, Type};
use postgres::{Client, NoTls, Row as PgRow};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;

use crate::run::{Backend, Db, DbError, DbErrorKind, Row};
use crate::shard::{fnv1a_64, PoolConfig, ShardId, LOGICAL_SHARDS};
use crate::value::SqlValue;

/// Postgres deadlock (`40P01`, `deadlock_detected`) and serialization failure (`40001`): the
/// server rolled the transaction back for a concurrency conflict, so the mutation path may
/// retry it. A `statement_timeout` cancel (`57014`) is not retried — re-running would just
/// time out again — so it stays [`Other`](DbErrorKind::Other) → an opaque `503`.
fn map_pg_err(e: postgres::Error) -> DbError {
    let kind = match e.code() {
        Some(c)
            if *c == SqlState::T_R_DEADLOCK_DETECTED
                || *c == SqlState::T_R_SERIALIZATION_FAILURE =>
        {
            DbErrorKind::Deadlock
        }
        _ => DbErrorKind::Other,
    };
    DbError::of(kind, e.to_string())
}

/// A pooled connection type: the `r2d2` manager over the sync `postgres` client, no TLS
/// (mirrors the MariaDB driver's TLS-off choice — no system OpenSSL dependency; a
/// deployment needing in-transit encryption re-enables it).
type PgManager = PostgresConnectionManager<NoTls>;
type PgPool = Pool<PgManager>;

// ---------- value conversion (pure, unit-tested) ---------------------------

/// A bound [`SqlValue`] rendered as a Postgres parameter. Numbers/bools bind natively;
/// every text-riding family (`text`/`uuid`/`timestamp`/`date`/`json`) rides as a String
/// encoded in **text format**, so Postgres coerces it into the inferred column type (uuid,
/// timestamptz, jsonb, …) exactly as it would a string literal — the runtime never needs
/// to know the column's Postgres type.
#[derive(Debug)]
pub(crate) enum PgValue {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    /// A string bound in text format; accepts the string-coercible OIDs (uuid/json/…).
    Text(String),
}

impl PgValue {
    pub(crate) fn from(v: &SqlValue) -> PgValue {
        match v {
            SqlValue::Null => PgValue::Null,
            SqlValue::Int(i) => PgValue::Int(*i),
            SqlValue::Float(f) => PgValue::Float(*f),
            SqlValue::Bool(b) => PgValue::Bool(*b),
            SqlValue::Text(s) => PgValue::Text(s.clone()),
            // `json` is serialized to its canonical text and coerced into `jsonb`.
            SqlValue::Json(j) => PgValue::Text(j.to_string()),
        }
    }
}

impl ToSql for PgValue {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        let _ = ty;
        match self {
            PgValue::Null => Ok(IsNull::Yes),
            // `bool` has no width ambiguity → its native binary encoding.
            PgValue::Bool(b) => b.to_sql(ty, out),
            // Numbers ride as **text**, like strings, so the server coerces them into the
            // *inferred* width (int2/int4/int8/numeric/float). A binary i64 would be 8 bytes
            // into whatever slot Postgres inferred — and an untyped integer literal (e.g. the
            // keyset guard's `:keyset_active = 0`) infers `int4`, so a binary i64 is rejected
            // (`22P03: incorrect binary data format`). Text sidesteps the width guess entirely.
            PgValue::Int(i) => {
                out.extend_from_slice(i.to_string().as_bytes());
                Ok(IsNull::No)
            }
            PgValue::Float(f) => {
                out.extend_from_slice(f.to_string().as_bytes());
                Ok(IsNull::No)
            }
            // A text-format string: write the UTF-8 bytes; the server coerces per the
            // inferred column type (`'…'::uuid` / `::jsonb` / …). See `encode_format`.
            PgValue::Text(s) => {
                out.extend_from_slice(s.as_bytes());
                Ok(IsNull::No)
            }
        }
    }

    fn accepts(_ty: &Type) -> bool {
        // A single bound value fills any inferred slot: numbers land on int/float/numeric,
        // bool on bool, a text-format string on uuid/timestamptz/jsonb/text/etc. We accept
        // broadly and let the *value* variant + text-format coercion do the right thing (the
        // planner already type-checked the arg against the column family, so a genuine
        // mismatch is caught before SQL runs).
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        match self {
            // Strings *and numbers* go in text format so Postgres applies string-literal
            // coercion into the inferred type (uuid/jsonb for strings; the inferred integer
            // width for numbers — sidestepping the binary-width mismatch, see `to_sql`).
            // `bool`/`null` use their native (binary) encoding.
            PgValue::Text(_) | PgValue::Int(_) | PgValue::Float(_) => Format::Text,
            PgValue::Bool(_) | PgValue::Null => Format::Binary,
        }
    }

    to_sql_checked!();
}

/// A returned column value → JSON (the shape the wire response is built from), read by the
/// column's Postgres type. Numbers map to JSON numbers; every string-family type
/// (text/uuid/timestamptz/date/jsonb) rides back as a JSON string. A genuinely
/// unknown/binary type falls back to lowercase hex (never a panic), matching the
/// MariaDB/SQLite drivers' `from_*`.
///
/// **rust-postgres returns results in *binary* format** (format code 1 for every column),
/// not text. For `text`/`varchar`/`json` the binary bytes *are* the UTF-8 text, so a raw
/// read yields the right string; but `uuid` (16 raw bytes), `timestamptz`/`timestamp` (an
/// i64 of microseconds since 2000-01-01), `date` (an i32 of days), and `jsonb` (a version
/// byte + text) all carry a binary layout that is not their canonical string. Those get an
/// explicit decoder here so they round-trip as the same string a text-format read (or a
/// literal) would — the value re-binds correctly (e.g. a keyset cursor's timestamp/uuid
/// basis compares equal on the next page). Decoders are pure + unit-tested below.
pub(crate) fn from_pg(row: &PgRow, idx: usize) -> serde_json::Value {
    use serde_json::Value as J;
    let col = row.columns()[idx].type_();
    match *col {
        Type::BOOL => opt(row.get::<_, Option<bool>>(idx), J::Bool),
        Type::INT2 => opt(row.get::<_, Option<i16>>(idx), |n| {
            J::Number((n as i64).into())
        }),
        Type::INT4 => opt(row.get::<_, Option<i32>>(idx), |n| {
            J::Number((n as i64).into())
        }),
        Type::INT8 => opt(row.get::<_, Option<i64>>(idx), |n| J::Number(n.into())),
        Type::FLOAT4 => opt(row.get::<_, Option<f32>>(idx), |f| {
            serde_json::Number::from_f64(f as f64).map_or(J::Null, J::Number)
        }),
        Type::FLOAT8 => opt(row.get::<_, Option<f64>>(idx), |f| {
            serde_json::Number::from_f64(f).map_or(J::Null, J::Number)
        }),
        // Binary layouts that are not their canonical string — decoded explicitly (see the
        // doc-comment). Read the raw field bytes (`PgBytes` accepts any OID) and format them.
        Type::UUID => opt(row.get::<_, Option<PgBytes>>(idx), |b| pg_uuid(&b.0)),
        Type::TIMESTAMPTZ | Type::TIMESTAMP => {
            opt(row.get::<_, Option<PgBytes>>(idx), |b| pg_timestamp(&b.0))
        }
        Type::DATE => opt(row.get::<_, Option<PgBytes>>(idx), |b| pg_date(&b.0)),
        Type::JSONB => opt(row.get::<_, Option<PgBytes>>(idx), |b| pg_jsonb(&b.0)),
        // `numeric`/`decimal` carries a packed base-10000 digit array, not its text — so a
        // decimal returns as its exact string (a JSON string, the wire form), losing no digit.
        Type::NUMERIC => opt(row.get::<_, Option<PgBytes>>(idx), |b| pg_numeric(&b.0)),
        // Everything else — text/varchar, `json`, etc. — has a binary form that *is* its
        // UTF-8 text, so read it straight as a String (a `FromSql` accepting any OID).
        _ => match row.try_get::<_, Option<PgText>>(idx) {
            Ok(Some(PgText(s))) => J::String(s),
            Ok(None) => J::Null,
            // A type we can't read as UTF-8 text (a raw binary column): fall back to hex of
            // the raw bytes so a request never panics on an exotic column.
            Err(_) => match row.try_get::<_, Option<PgBytes>>(idx) {
                Ok(Some(PgBytes(b))) => J::String(hex(&b)),
                _ => J::Null,
            },
        },
    }
}

/// Postgres's binary temporal epoch: 2000-01-01 is 10957 days after the Unix epoch, the
/// offset that turns "days since 2000" into the days-since-1970 the civil conversion uses.
const PG_EPOCH_DAYS_FROM_UNIX: i64 = 10957;
const MICROS_PER_DAY: i64 = 86_400_000_000;

/// A binary `uuid` (16 raw bytes) → the canonical hyphenated `8-4-4-4-12` string. A hex fall
/// back (via [`from_pg`]) would drop the hyphens — a technically-parseable but non-canonical
/// form; this emits the real thing.
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
    let (y, m, d) = civil_from_days(days as i64 + PG_EPOCH_DAYS_FROM_UNIX);
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
    let weight = rd(2) as i32;
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
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era, [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year (Mar-based), [0, 365]
    let mp = (5 * doy + 2) / 153; // Mar-based month, [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // day, [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // Jan-based month, [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Apply `f` to a fetched `Option<T>`, mapping `None` (SQL NULL) to JSON null.
fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> serde_json::Value) -> serde_json::Value {
    v.map_or(serde_json::Value::Null, f)
}

/// A `FromSql` that reads *any* column's text representation into a `String`. Postgres sends
/// results in text format by default, so uuid/timestamptz/date/json all arrive as their
/// canonical text — this pulls that text out regardless of the column's declared type.
struct PgText(String);

impl<'a> postgres::types::FromSql<'a> for PgText {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> Result<PgText, Box<dyn std::error::Error + Sync + Send>> {
        Ok(PgText(String::from_utf8(raw.to_vec())?))
    }
    fn accepts(_ty: &Type) -> bool {
        true
    }
}

/// A `FromSql` fallback that reads the raw bytes of a column that isn't valid UTF-8 text
/// (a genuinely binary type), so `from_pg` can hex-encode it rather than panic.
struct PgBytes(Vec<u8>);

impl<'a> postgres::types::FromSql<'a> for PgBytes {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> Result<PgBytes, Box<dyn std::error::Error + Sync + Send>> {
        Ok(PgBytes(raw.to_vec()))
    }
    fn accepts(_ty: &Type) -> bool {
        true
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

/// One pooled Postgres connection, running one request. Checked out of a shard's pool for
/// the request's duration and returned on drop (the pool recycles it).
pub struct PostgresDb {
    conn: r2d2::PooledConnection<PgManager>,
}

impl PostgresDb {
    /// Wrap an already-checked-out connection (the router hands these out).
    pub fn new(conn: r2d2::PooledConnection<PgManager>) -> PostgresDb {
        PostgresDb { conn }
    }

    /// Borrow the bound values as `&dyn ToSql` params (the `postgres` API's shape).
    fn params(bound: &[PgValue]) -> Vec<&(dyn ToSql + Sync)> {
        bound.iter().map(|v| v as &(dyn ToSql + Sync)).collect()
    }
}

impl Db for PostgresDb {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>, DbError> {
        let bound: Vec<PgValue> = params.iter().map(PgValue::from).collect();
        let rows = self
            .conn
            .query(sql, &Self::params(&bound))
            .map_err(map_pg_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let cols = row.columns();
            let mut obj = serde_json::Map::with_capacity(cols.len());
            for (i, col) in cols.iter().enumerate() {
                // The SELECT aliases each projection to its output name, so a row is already
                // the response object.
                obj.insert(col.name().to_string(), from_pg(row, i));
            }
            out.push(obj);
        }
        Ok(out)
    }

    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        let bound: Vec<PgValue> = params.iter().map(PgValue::from).collect();
        self.conn
            .execute(sql, &Self::params(&bound))
            .map_err(map_pg_err)
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.conn.batch_execute("BEGIN").map_err(map_pg_err)
    }
    fn commit(&mut self) -> Result<(), DbError> {
        self.conn.batch_execute("COMMIT").map_err(map_pg_err)
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn.batch_execute("ROLLBACK").map_err(map_pg_err)
    }
}

// ---------- the shard router ------------------------------------------------

/// Routes each request to exactly one physical Postgres shard's connection pool — the
/// [`crate::driver::ShardRouter`] twin. Holds one bounded `r2d2` pool per shard and the
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
    pub fn new(urls: &[String], pool: PoolConfig) -> Result<PgRouter, DbError> {
        if urls.is_empty() {
            return Err(DbError::new("shard router needs at least one database url"));
        }
        let shards = urls
            .iter()
            .map(|u| build_pool(u, pool))
            .collect::<Result<Vec<_>, _>>()?;
        let n = shards.len();
        let assign = (0..LOGICAL_SHARDS).map(|i| i % n).collect();
        Ok(PgRouter { shards, assign })
    }

    /// The common case: one physical shard (all logical shards map to it). The router is
    /// still the seam — splitting later is a config change, not a code change.
    pub fn single(url: &str, pool: PoolConfig) -> Result<PgRouter, DbError> {
        PgRouter::new(std::slice::from_ref(&url.to_string()), pool)
    }

    /// The physical shard a key routes to: a stable logical hash, then the assignment.
    pub fn shard_for(&self, key: &str) -> ShardId {
        let logical = (fnv1a_64(key.as_bytes()) % LOGICAL_SHARDS as u64) as usize;
        self.assign[logical]
    }

    /// Check out a connection to the shard a key routes to (single-shard dispatch).
    pub fn checkout(&self, key: &str) -> Result<PostgresDb, DbError> {
        self.checkout_shard(self.shard_for(key))
    }

    /// Check out a connection to a specific physical shard. `r2d2` waits at most the pool's
    /// configured `connection_timeout` (from [`PoolConfig::checkout_timeout`]) for a free
    /// connection, then errors — a saturated pool becomes a fast, retryable pool-exhausted
    /// `503`, never a hung worker.
    pub fn checkout_shard(&self, shard: ShardId) -> Result<PostgresDb, DbError> {
        let pool = self
            .shards
            .get(shard)
            .ok_or_else(|| DbError::new(format!("no shard {shard}")))?;
        let conn = pool.get().map_err(|e: r2d2::Error| {
            DbError::of(
                DbErrorKind::PoolExhausted,
                format!("connection pool exhausted: {e}"),
            )
        })?;
        Ok(PostgresDb::new(conn))
    }

    /// How many physical shards the router spans.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

/// The router is the Postgres [`Backend`]: it checks out a pooled [`PostgresDb`] for the
/// key's shard. The HTTP edge depends only on this trait, so it is a drop-in beside the
/// MariaDB `ShardRouter` with no change to `based serve` (the `Db` seam is dialect-agnostic;
/// only the `Compiled.dialect` must match the backend, a deployment invariant).
impl Backend for PgRouter {
    fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(PgRouter::checkout(self, shard_key)?))
    }

    /// Readiness = every physical shard's pool can hand out a connection that answers
    /// `SELECT 1` (a stale pooled socket is caught, not just a checkout) — the same
    /// all-shards-ready rule as the MariaDB router.
    fn ping(&self) -> Result<(), DbError> {
        for shard in 0..self.shard_count() {
            let mut db = self.checkout_shard(shard)?;
            db.fetch("SELECT 1", &[])?;
        }
        Ok(())
    }
}

/// Build one shard's bounded connection pool from a `postgres://…` URL. The pool's
/// `connection_timeout` is the checkout wait (pool-exhaustion → fast `503`), and the
/// server-side `statement_timeout` is set as a startup option so every statement on a
/// pooled connection is capped — a runaway query is cancelled (`57014`) rather than hanging.
fn build_pool(url: &str, cfg: PoolConfig) -> Result<PgPool, DbError> {
    let mut config = url
        .parse::<postgres::Config>()
        .map_err(|e| DbError::new(format!("bad database url: {e}")))?;
    if cfg.statement_timeout > Duration::ZERO {
        // Startup `options` apply the timeout to every statement on the connection (ms).
        let ms = cfg.statement_timeout.as_millis();
        config.options(&format!("-c statement_timeout={ms}"));
    }
    let manager = PostgresConnectionManager::new(config, NoTls);
    Pool::builder()
        .min_idle(Some(cfg.min as u32))
        .max_size(cfg.max as u32)
        .connection_timeout(cfg.checkout_timeout)
        .build(manager)
        .map_err(|e| DbError::new(format!("connecting to shard: {e}")))
}

/// A convenience one-shot [`Client`] over a URL, no pool — used for test setup (`CREATE
/// TABLE`, seeding) where a pool is overkill. Not on the serving hot path.
pub fn connect(url: &str) -> Result<Client, DbError> {
    let config = url
        .parse::<postgres::Config>()
        .map_err(|e| DbError::new(format!("bad database url: {e}")))?;
    config
        .connect(NoTls)
        .map_err(|e| DbError::new(format!("connecting: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `PgValue` encodes its bytes and reports the right null/format — the pure mapping
    /// (the live round-trip is proven in `tests/postgres_integration.rs`).
    #[test]
    fn pgvalue_from_sqlvalue_families() {
        assert!(matches!(PgValue::from(&SqlValue::Null), PgValue::Null));
        assert!(matches!(PgValue::from(&SqlValue::Int(7)), PgValue::Int(7)));
        assert!(matches!(
            PgValue::from(&SqlValue::Bool(true)),
            PgValue::Bool(true)
        ));
        match PgValue::from(&SqlValue::Text(
            "00000000-0000-4000-8000-0000000000a1".into(),
        )) {
            PgValue::Text(s) => assert_eq!(s, "00000000-0000-4000-8000-0000000000a1"),
            _ => panic!("uuid text should map to PgValue::Text"),
        }
        // json is serialized to canonical text (coerced into `jsonb` server-side).
        match PgValue::from(&SqlValue::Json(json!({ "a": 1 }))) {
            PgValue::Text(s) => assert_eq!(s, r#"{"a":1}"#),
            _ => panic!("json should map to PgValue::Text"),
        }
    }

    #[test]
    fn text_values_encode_in_text_format() {
        // Strings *and numbers* bind in text format so Postgres coerces each into its inferred
        // type — uuid/jsonb for strings, the inferred integer width for numbers (a binary i64
        // is rejected against an inferred `int4`, e.g. the keyset `= 0` guard). `bool` keeps
        // native binary encoding. (`Format` isn't `PartialEq`, so we match on its variant.)
        assert!(matches!(
            PgValue::Text("x".into()).encode_format(&Type::UUID),
            Format::Text
        ));
        assert!(matches!(
            PgValue::Int(1).encode_format(&Type::INT4),
            Format::Text
        ));
        assert!(matches!(
            PgValue::Float(1.5).encode_format(&Type::FLOAT8),
            Format::Text
        ));
        assert!(matches!(
            PgValue::Bool(true).encode_format(&Type::BOOL),
            Format::Binary
        ));
    }

    #[test]
    fn number_values_write_decimal_text() {
        // Int/Float render their decimal text (the bytes Postgres coerces into the inferred
        // width), never a binary payload — so an i64 bind never mismatches an `int4` slot.
        let mut buf = BytesMut::new();
        assert!(matches!(
            PgValue::Int(-42).to_sql(&Type::INT4, &mut buf).unwrap(),
            IsNull::No
        ));
        assert_eq!(&buf[..], b"-42");
        let mut fbuf = BytesMut::new();
        PgValue::Float(1.5)
            .to_sql(&Type::FLOAT8, &mut fbuf)
            .unwrap();
        assert_eq!(&fbuf[..], b"1.5");
    }

    #[test]
    fn null_reports_is_null() {
        // The null variant serializes as SQL NULL regardless of the inferred column type.
        let mut buf = BytesMut::new();
        let n = PgValue::Null.to_sql(&Type::UUID, &mut buf).unwrap();
        assert!(matches!(n, IsNull::Yes));
    }

    #[test]
    fn text_value_writes_utf8_bytes() {
        let mut buf = BytesMut::new();
        let n = PgValue::Text("abc".into())
            .to_sql(&Type::UUID, &mut buf)
            .unwrap();
        assert!(matches!(n, IsNull::No));
        assert_eq!(&buf[..], b"abc");
    }

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
    /// the fix for reading a `uuid`/`timestamptz`/`date`/`jsonb` result column (rust-postgres
    /// returns results in *binary* format, so a raw text read mangles these). Proven live in
    /// `tests/postgres_integration.rs`; here the pure byte→string mapping is unit-covered.
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
}
