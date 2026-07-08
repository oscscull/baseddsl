//! The SQLite [`Db`] + [`Backend`] (feature `sqlite`).
//!
//! SQLite is the **infra-free** backend: an in-memory (or file) database that needs no
//! live server, so it unlocks real end-to-end integration tests against a genuine engine
//! — the whole `plan → run → shape` path exercised over an actual `Db`, not a `MockDb`
//! (see `tests/sqlite_integration.rs`). It is also the lowest-friction second dialect to
//! land: SQLite binds positional `?` exactly like MariaDB (D21 — no dialect-aware scanner
//! change, unlike Postgres's `$n`), and it accepts the DML the runtime executes
//! (backtick-quoted identifiers, `= TRUE`, `IS NULL`, `LIMIT`, multi-table joins) as-is.
//!
//! Two pieces, mirroring the MariaDB driver ([`crate::driver`]):
//! - [`SqliteDb`] — a [`Db`] over one shared connection. Every method locks the
//!   connection (SQLite serializes writes anyway) and maps [`SqlValue`] binds to
//!   `rusqlite` params and result columns back to JSON.
//! - [`SqliteBackend`] — the [`Backend`]. A single-file/in-memory database has no shards,
//!   so it ignores the shard key and hands every checkout the *same* connection (an
//!   in-memory DB is per-connection, so sharing one is what lets separate requests see
//!   each other's writes — exactly the integration-test need). `ping` runs `SELECT 1`.
//!
//! It reuses the `rusqlite` crate (bundled SQLite, no system dependency — principle 7).
//! Concurrency: the shared connection is behind a `Mutex`, so checkouts serialize on it.
//! That is correct for SQLite (a file DB serializes writers regardless) and keeps an
//! in-memory DB coherent across the worker pool; a throughput-hungry deployment would use
//! MariaDB (D20's scale-out model), not many SQLite connections.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::types::{Value as SqlV, ValueRef};
use rusqlite::{Connection, ErrorCode};

use crate::run::{Backend, Db, DbError, DbErrorKind, Row};
use crate::value::SqlValue;

/// How long a locked SQLite database waits for the lock to clear before returning
/// `SQLITE_BUSY` (D65). SQLite serializes writers, so under a concurrent writer (another
/// process on a file DB) a checkout would otherwise fail instantly; the busy timeout lets it
/// wait a bounded moment, and a genuine timeout then surfaces as a retryable deadlock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One shared SQLite connection behind a `Mutex`. Cloning a [`SqliteDb`] (via the
/// backend handing out clones) shares the same underlying connection, so an in-memory
/// database stays coherent across requests and a `tx` sees its own writes.
pub struct SqliteDb {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteDb {
    /// Wrap an already-open connection as a shareable [`Db`]. Sets a bounded `busy_timeout`
    /// (D65) so a momentarily-locked database waits rather than failing a writer instantly.
    pub fn new(conn: Connection) -> SqliteDb {
        // Best effort: a driver that rejects the pragma still works, just without the wait.
        let _ = conn.busy_timeout(BUSY_TIMEOUT);
        SqliteDb {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Share the same underlying connection (what the backend hands out per checkout).
    fn share(&self) -> SqliteDb {
        SqliteDb {
            conn: Arc::clone(&self.conn),
        }
    }
}

/// Bind a slice of [`SqlValue`]s as positional `rusqlite` params.
fn to_params(params: &[SqlValue]) -> Vec<SqlV> {
    params.iter().map(to_sqlite).collect()
}

/// A bound [`SqlValue`] → `rusqlite`'s owned value. Families line up with `SqlValue`'s
/// (D1): a `bool` binds as SQLite's integer `0/1`; `json` is sent as its serialized text
/// (SQLite has no JSON type — it stores JSON as `TEXT`, which is how the wire reads it back).
fn to_sqlite(v: &SqlValue) -> SqlV {
    match v {
        SqlValue::Null => SqlV::Null,
        SqlValue::Int(i) => SqlV::Integer(*i),
        SqlValue::Float(f) => SqlV::Real(*f),
        SqlValue::Bool(b) => SqlV::Integer(*b as i64),
        SqlValue::Text(s) => SqlV::Text(s.clone()),
        SqlValue::Json(j) => SqlV::Text(j.to_string()),
    }
}

/// A returned column value → JSON (the shape the wire response is built from). Text/uuid/
/// json/timestamp all ride the wire as strings (D1), so `Text` maps straight through; a
/// genuinely binary `BLOB` falls back to lowercase hex (never a panic, matching the
/// MariaDB driver's `from_mysql`).
fn from_sqlite(v: ValueRef<'_>) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        ValueRef::Null => J::Null,
        ValueRef::Integer(i) => J::Number(i.into()),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(J::Null, J::Number),
        ValueRef::Text(b) => match std::str::from_utf8(b) {
            Ok(s) => J::String(s.to_string()),
            Err(_) => J::String(hex(b)),
        },
        ValueRef::Blob(b) => J::String(hex(b)),
    }
}

/// Lowercase hex of a byte slice (for a non-UTF-8 / blob column value).
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

impl Db for SqliteDb {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>, DbError> {
        let conn = self.conn.lock().map_err(poisoned)?;
        let mut stmt = conn.prepare(sql).map_err(dberr)?;
        // Capture the column names up front (the SELECT aliases each projection to its
        // output name, so a row is already the response object).
        let names: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let bound = to_params(params);
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            bound.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        let mut result = Vec::new();
        let mut cursor = stmt.query(param_refs.as_slice()).map_err(dberr)?;
        while let Some(row) = cursor.next().map_err(dberr)? {
            let mut obj = serde_json::Map::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                obj.insert(name.clone(), from_sqlite(row.get_ref(i).map_err(dberr)?));
            }
            result.push(obj);
        }
        Ok(result)
    }

    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        let conn = self.conn.lock().map_err(poisoned)?;
        let bound = to_params(params);
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            bound.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        let n = conn.execute(sql, param_refs.as_slice()).map_err(dberr)?;
        Ok(n as u64)
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.conn
            .lock()
            .map_err(poisoned)?
            .execute_batch("BEGIN")
            .map_err(dberr)
    }
    fn commit(&mut self) -> Result<(), DbError> {
        self.conn
            .lock()
            .map_err(poisoned)?
            .execute_batch("COMMIT")
            .map_err(dberr)
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn
            .lock()
            .map_err(poisoned)?
            .execute_batch("ROLLBACK")
            .map_err(dberr)
    }
}

/// The SQLite [`Backend`]: one shared connection, no shards. It hands every checkout the
/// *same* connection (ignoring the shard key), so an in-memory database stays coherent
/// across requests — the property that makes it a real integration-test engine. A file
/// database works the same way; SQLite serializes writers itself.
pub struct SqliteBackend {
    db: SqliteDb,
}

impl SqliteBackend {
    /// A backend over a fresh in-memory database (`:memory:`). Every checkout shares it,
    /// so writes from one request are visible to the next — no infra, no file.
    pub fn in_memory() -> Result<SqliteBackend, DbError> {
        let conn = Connection::open_in_memory().map_err(dberr)?;
        Ok(SqliteBackend {
            db: SqliteDb::new(conn),
        })
    }

    /// A backend over a file database at `path` (created if absent).
    pub fn open(path: &str) -> Result<SqliteBackend, DbError> {
        let conn = Connection::open(path).map_err(dberr)?;
        Ok(SqliteBackend {
            db: SqliteDb::new(conn),
        })
    }

    /// Run setup SQL (e.g. `CREATE TABLE …; INSERT …;`) against the shared database.
    /// A test seeds its schema + fixtures through this before dispatching requests.
    pub fn execute_batch(&self, sql: &str) -> Result<(), DbError> {
        self.db
            .conn
            .lock()
            .map_err(poisoned)?
            .execute_batch(sql)
            .map_err(dberr)
    }
}

// A single shared connection behind a `Mutex` is `Send + Sync`, so the backend satisfies
// the `Backend: Send + Sync` bound the HTTP worker pool needs (checkouts serialize on it).
impl Backend for SqliteBackend {
    fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        // Single database: the shard key is ignored (SQLite has no shards). Every checkout
        // shares the one connection, so an in-memory DB's writes persist across requests.
        Ok(Box::new(self.db.share()))
    }

    fn ping(&self) -> Result<(), DbError> {
        // A trivial round-trip validates the connection is usable right now.
        self.db.share().fetch("SELECT 1", &[]).map(|_| ())
    }
}

/// Map a `rusqlite` error to the runtime's [`DbError`] (→ the wire's retryable `503`). A
/// `SQLITE_BUSY`/`SQLITE_LOCKED` (a writer lost the lock race after the busy timeout) is a
/// deadlock-class failure the mutation path may retry (D65); everything else is opaque.
fn dberr(e: rusqlite::Error) -> DbError {
    let kind = match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) =>
        {
            DbErrorKind::Deadlock
        }
        _ => DbErrorKind::Other,
    };
    DbError::of(kind, e.to_string())
}

/// A poisoned connection mutex (a prior holder panicked mid-statement) is an operational
/// failure like any other — surface it as a `DbError`, never a panic-through.
fn poisoned<T>(_: std::sync::PoisonError<T>) -> DbError {
    DbError::new("sqlite connection mutex poisoned")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sqlvalue_to_sqlite_families() {
        assert_eq!(to_sqlite(&SqlValue::Null), SqlV::Null);
        assert_eq!(to_sqlite(&SqlValue::Int(7)), SqlV::Integer(7));
        assert_eq!(to_sqlite(&SqlValue::Float(1.5)), SqlV::Real(1.5));
        // bool rides as integer 0/1.
        assert_eq!(to_sqlite(&SqlValue::Bool(true)), SqlV::Integer(1));
        assert_eq!(to_sqlite(&SqlValue::Bool(false)), SqlV::Integer(0));
        assert_eq!(
            to_sqlite(&SqlValue::Text("o-1".into())),
            SqlV::Text("o-1".into())
        );
        // json is sent as serialized text.
        assert_eq!(
            to_sqlite(&SqlValue::Json(json!({ "a": 1 }))),
            SqlV::Text(r#"{"a":1}"#.into())
        );
    }

    #[test]
    fn sqlite_value_to_json() {
        use serde_json::Value as J;
        assert_eq!(from_sqlite(ValueRef::Null), J::Null);
        assert_eq!(from_sqlite(ValueRef::Integer(42)), json!(42));
        assert_eq!(from_sqlite(ValueRef::Real(2.5)), json!(2.5));
        // text/uuid ride back as strings.
        assert_eq!(from_sqlite(ValueRef::Text(b"paid")), json!("paid"));
        // a genuinely binary blob falls back to hex, never a panic.
        assert_eq!(from_sqlite(ValueRef::Blob(&[0xff, 0x01])), json!("ff01"));
    }

    #[test]
    fn round_trips_a_write_then_read() {
        // The seam works end to end against a real engine, no mock: create a table,
        // execute an INSERT with bound params, fetch it back shaped as JSON.
        let mut db = SqliteBackend::in_memory().unwrap().checkout("").unwrap();
        db.execute("CREATE TABLE t (id TEXT, n INTEGER, ok BOOLEAN)", &[])
            .unwrap();
        db.execute(
            "INSERT INTO t (id, n, ok) VALUES (?, ?, ?)",
            &[
                SqlValue::Text("a".into()),
                SqlValue::Int(5),
                SqlValue::Bool(true),
            ],
        )
        .unwrap();
        let rows = db
            .fetch(
                "SELECT id, n, ok FROM t WHERE id = ?",
                &[SqlValue::Text("a".into())],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], json!("a"));
        assert_eq!(rows[0]["n"], json!(5));
        assert_eq!(rows[0]["ok"], json!(1));
    }

    #[test]
    fn shared_connection_persists_across_checkouts() {
        // An in-memory DB is per-connection; the backend shares one connection, so a
        // write via one checkout is visible via the next (the integration-test property).
        let backend = SqliteBackend::in_memory().unwrap();
        backend.execute_batch("CREATE TABLE t (id TEXT)").unwrap();
        backend
            .checkout("")
            .unwrap()
            .execute("INSERT INTO t (id) VALUES ('x')", &[])
            .unwrap();
        let rows = backend
            .checkout("")
            .unwrap()
            .fetch("SELECT id FROM t", &[])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], json!("x"));
    }

    #[test]
    fn ping_ok_on_a_live_db() {
        assert!(SqliteBackend::in_memory().unwrap().ping().is_ok());
    }

    #[test]
    fn busy_lock_is_classified_as_a_retryable_deadlock() {
        // A real SQLite `SQLITE_BUSY` maps to the deadlock kind (D65) so the mutation path
        // retries it. Two connections to one file DB: A holds a write lock, B (busy_timeout
        // 0, so it fails immediately rather than waiting) hits BUSY on its own write lock.
        let path = std::env::temp_dir().join(format!("based_busy_{}.db", std::process::id()));
        let path_str = path.to_str().unwrap();
        let a = Connection::open(path_str).unwrap();
        a.execute_batch("CREATE TABLE IF NOT EXISTS t (id INTEGER)")
            .unwrap();
        a.execute_batch("BEGIN IMMEDIATE").unwrap(); // A now holds the reserved write lock

        let b = Connection::open(path_str).unwrap();
        b.busy_timeout(Duration::ZERO).unwrap(); // don't wait — surface BUSY at once
        let err = b.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert_eq!(
            dberr(err).kind,
            DbErrorKind::Deadlock,
            "SQLITE_BUSY should be a retryable deadlock-class failure"
        );

        let _ = a.execute_batch("ROLLBACK");
        drop((a, b));
        let _ = std::fs::remove_file(&path);
    }
}
