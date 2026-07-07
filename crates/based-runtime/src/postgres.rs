//! The concrete Postgres driver + shard router (feature `postgres`, A3/D38).
//!
//! This is the Postgres twin of the MariaDB driver ([`crate::driver`]): the production
//! [`Db`]/[`Backend`] that runs the *verbatim* Postgres-lowered SQL (D29) against a real
//! server. Postgres codegen (`ddl`/`dml`/`mutations`) and the dialect-aware `:name`→`$n`
//! scanner are already done (D29); this is the runtime that executes what they emit. Two
//! layers, exactly mirroring the MariaDB structure:
//!
//! - [`PostgresDb`] — a [`Db`] over one pooled connection. It runs the whole request on
//!   that single connection (a mutation's `tx` must see its own writes), converting
//!   [`SqlValue`] binds to Postgres parameters and returned columns back to JSON. It reuses
//!   the pure-Rust **synchronous** `postgres` crate (principle 7, and D20's sync model — no
//!   async runtime) rather than hand-rolling the protocol.
//!
//! - [`PgRouter`] — the scale-out seam, the [`crate::driver::ShardRouter`] twin. One
//!   **bounded** `r2d2` connection pool per physical shard, single-shard dispatch by the
//!   same stable FNV logical-shard hash (no scatter-gather → a `tx` is one shard, no
//!   distributed transaction; add capacity without rehashing keys — the Vitess/Citus model).
//!
//! **The value-mapping subtlety (Postgres-specific).** The runtime is dialect-neutral: a
//! `uuid`/`timestamptz`/`jsonb` value is carried as [`SqlValue::Text`] (a String — on the
//! wire these are all strings, D1). Postgres, unlike MySQL/SQLite, *infers* each `$n`
//! parameter's type from the column it binds against and, in the extended protocol, refuses
//! a `text`-encoded Rust `String` for an inferred `uuid`/`jsonb` OID. So [`PgValue`] is a
//! `ToSql` newtype that (a) `accepts` those non-text OIDs and (b) encodes its bytes in
//! **text format** ([`Format::Text`]) — the server then applies its normal string-literal
//! coercion (the same path `'…'::uuid` takes). This keeps the runtime free of per-column
//! Postgres types while round-tripping every family. The mapping is pure and unit-tested
//! below (like `from_mysql`); connecting/executing can only be compile-verified here and is
//! proven by `tests/postgres_integration.rs` against a live server.

use bytes::BytesMut;
use postgres::types::{to_sql_checked, Format, IsNull, ToSql, Type};
use postgres::{Client, NoTls, Row as PgRow};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;

use crate::run::{Backend, Db, DbError, Row};
use crate::shard::{fnv1a_64, PoolConfig, ShardId, LOGICAL_SHARDS};
use crate::value::SqlValue;

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
/// (text/uuid/timestamptz/date/json) rides back as a JSON string (D1) via the column's text
/// representation. A genuinely unknown/binary type falls back to lowercase hex (never a
/// panic), matching the MariaDB/SQLite drivers' `from_*`.
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
        // Everything else — text/varchar, uuid, timestamptz/timestamp, date, json/jsonb — is
        // read as its text representation (a `FromSql` that accepts any OID and hands back the
        // raw column text), so it rides the wire as a JSON string. This mirrors the MariaDB
        // driver, where uuid/timestamp/json all come back as strings.
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
            .map_err(|e| DbError::new(e.to_string()))?;
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
            .map_err(|e| DbError::new(e.to_string()))
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.conn
            .batch_execute("BEGIN")
            .map_err(|e| DbError::new(e.to_string()))
    }
    fn commit(&mut self) -> Result<(), DbError> {
        self.conn
            .batch_execute("COMMIT")
            .map_err(|e| DbError::new(e.to_string()))
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn
            .batch_execute("ROLLBACK")
            .map_err(|e| DbError::new(e.to_string()))
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

    /// Check out a connection to a specific physical shard.
    pub fn checkout_shard(&self, shard: ShardId) -> Result<PostgresDb, DbError> {
        let pool = self
            .shards
            .get(shard)
            .ok_or_else(|| DbError::new(format!("no shard {shard}")))?;
        let conn = pool
            .get()
            .map_err(|e: r2d2::Error| DbError::new(e.to_string()))?;
        Ok(PostgresDb::new(conn))
    }

    /// How many physical shards the router spans.
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

/// The router is the Postgres [`Backend`]: it checks out a pooled [`PostgresDb`] for the
/// key's shard. The HTTP edge depends only on this trait, so it is a drop-in beside the
/// MariaDB `ShardRouter` with no change to `based serve` (D21 — the `Db` seam is dialect-
/// agnostic; only the `Compiled.dialect` must match the backend, a deployment invariant).
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

/// Build one shard's bounded connection pool from a `postgres://…` URL.
fn build_pool(url: &str, cfg: PoolConfig) -> Result<PgPool, DbError> {
    let config = url
        .parse::<postgres::Config>()
        .map_err(|e| DbError::new(format!("bad database url: {e}")))?;
    let manager = PostgresConnectionManager::new(config, NoTls);
    Pool::builder()
        .min_idle(Some(cfg.min as u32))
        .max_size(cfg.max as u32)
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
}
