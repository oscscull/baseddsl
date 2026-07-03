//! Executing a planned query and shaping the rows into the response envelope.
//!
//! The concrete MariaDB driver is the next slice; execution goes through the
//! abstract [`Db`] trait — the runtime's twin of the generated client's abstract
//! `Transport`. A [`MockDb`] returns canned rows so the whole request → JSON path
//! is testable with no database. Row shaping is where the envelope becomes real:
//! `get` → a JSON object or `null`, `list` → an array, a paginated `list` → the
//! `{ rows, cursor }` page envelope (cursor encoding is a driver concern, deferred —
//! it rides as `null` here).

use crate::id::IdGen;
use crate::load::Compiled;
use crate::plan::{
    plan_mutation, plan_query, Envelope, MutationPlan, PlanError, QueryPlan, Request, Stmt,
};
use crate::value::SqlValue;

/// One returned row: column alias → JSON value (the SELECT aliases each projection
/// to its output name, so a row is already the response object).
pub type Row = serde_json::Map<String, serde_json::Value>;

/// A failure from the database itself — connection lost, timeout, deadlock, a shard
/// down, pool exhausted. Distinct from a [`PlanError`] (a boundary/validation failure
/// *before* any SQL): a `DbError` is an operational failure the wire maps to a
/// retryable `503`. The message is human-facing; the driver fills it from its error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    pub message: String,
}

impl DbError {
    pub fn new(message: impl Into<String>) -> DbError {
        DbError {
            message: message.into(),
        }
    }
}

/// Why running a request failed: a boundary [`PlanError`] (bad/missing input, unknown
/// callable — the caller can fix it) or a [`DbError`] (the database failed — an
/// operational, retryable failure). The wire (`serve`) maps each to its HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Plan(PlanError),
    Db(DbError),
}

impl From<PlanError> for RunError {
    fn from(e: PlanError) -> RunError {
        RunError::Plan(e)
    }
}
impl From<DbError> for RunError {
    fn from(e: DbError) -> RunError {
        RunError::Db(e)
    }
}

/// The database seam. The runtime hands it positional SQL + values; the read path
/// `fetch`es rows, the write path `execute`s statements under an engine-owned
/// transaction (principle 7 — the engine, not the emitted SQL, owns BEGIN/COMMIT).
/// Every method is **fallible**: a dependable driver surfaces connection/query
/// failures rather than panicking (the concrete `MariaDb` under the `mariadb`
/// feature). The write methods default so a read-only [`Db`] need not implement them.
pub trait Db {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>, DbError>;

    /// Execute one write statement (INSERT/UPDATE/DELETE); returns rows affected.
    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        let _ = (sql, params);
        Ok(0)
    }
    /// Open the transaction the whole mutation body runs in.
    fn begin(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    /// Commit it (all writes succeeded).
    fn commit(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    /// Roll it back (a write failed). Best-effort: called on the error path, its own
    /// failure must not mask the original error.
    fn rollback(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

/// Plan and run a query request, returning the shaped JSON response.
pub fn run_query(
    compiled: &Compiled,
    db: &mut dyn Db,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    let plan = plan_query(compiled, req)?;
    Ok(shape(db, &plan)?)
}

/// Plan and run a mutation request: id-gen + bind, then execute every write under one
/// engine-owned transaction, returning the write response.
pub fn run_mutation(
    compiled: &Compiled,
    db: &mut dyn Db,
    id_gen: &mut dyn IdGen,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    let plan = plan_mutation(compiled, req, id_gen)?;
    Ok(apply(db, &plan)?)
}

/// Execute a mutation plan's writes in order under one transaction, then assemble the
/// write response. If any write fails the transaction is rolled back and the error
/// surfaced — a mutation is all-or-nothing, never a partial write (principle 7,
/// dependability). The declared-shape re-select (RETURNING vs. re-select, D12) is
/// deferred: the response identifies the created row by its engine-generated `id`
/// (`{ "id": … }`), or is empty for a mutation that creates nothing.
fn apply(db: &mut dyn Db, plan: &MutationPlan) -> Result<serde_json::Value, DbError> {
    use serde_json::Value as J;
    db.begin()?;
    for stmt in &plan.stmts {
        if let Err(e) = db.execute(&stmt.sql, &stmt.params) {
            // Best-effort rollback; surface the original write error, not a
            // rollback failure (the connection may already be gone).
            let _ = db.rollback();
            return Err(e);
        }
    }
    db.commit()?;
    Ok(match &plan.result_id {
        Some(id) => {
            let mut obj = serde_json::Map::new();
            obj.insert("id".into(), J::String(id.clone()));
            J::Object(obj)
        }
        None => J::Object(serde_json::Map::new()),
    })
}

/// Execute a plan's statements and assemble the response per its envelope.
fn shape(db: &mut dyn Db, plan: &QueryPlan) -> Result<serde_json::Value, DbError> {
    use serde_json::Value as J;
    let rows = run_stmt(db, &plan.main)?;
    Ok(match plan.envelope {
        // `get`: the first row, or JSON null (Option<T>).
        Envelope::One => rows.into_iter().next().map(J::Object).unwrap_or(J::Null),
        // `list`: every row as an array.
        Envelope::Many => J::Array(rows.into_iter().map(J::Object).collect()),
        // paginated `list`: the { rows, cursor } envelope (cursor deferred → null),
        // plus `total` when the query asked for a count.
        Envelope::Page { with_count } => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "rows".into(),
                J::Array(rows.into_iter().map(J::Object).collect()),
            );
            obj.insert("cursor".into(), J::Null);
            if with_count {
                if let Some(count) = &plan.count {
                    let total = run_stmt(db, count)?
                        .into_iter()
                        .next()
                        .and_then(|mut r| r.remove("count"))
                        .unwrap_or(J::Null);
                    obj.insert("total".into(), total);
                }
            }
            J::Object(obj)
        }
    })
}

fn run_stmt(db: &mut dyn Db, stmt: &Stmt) -> Result<Vec<Row>, DbError> {
    db.fetch(&stmt.sql, &stmt.params)
}

/// A test double: returns pre-loaded row batches in call order, recording every
/// `(sql, params)` it was asked to run (both `fetch` and `execute`) so tests can
/// assert the bound statements, plus the transaction boundaries it saw. Set `fail`
/// to make every `fetch`/`execute` return a [`DbError`] (the driver-failure path).
#[derive(Default)]
pub struct MockDb {
    /// Row batches, popped front-to-back per `fetch` call.
    pub responses: std::collections::VecDeque<Vec<Row>>,
    /// Every executed statement, in order — `fetch` and `execute` alike (for assertions).
    pub calls: Vec<(String, Vec<SqlValue>)>,
    /// `begin`/`commit`/`rollback` seen, in order (write-path transaction assertions).
    pub tx: Vec<&'static str>,
    /// When set, `fetch`/`execute` return this as a [`DbError`] (simulate a DB fault).
    pub fail: Option<String>,
}

impl MockDb {
    /// A mock that replies to each `fetch` with the given batches, in order.
    pub fn new(responses: Vec<Vec<Row>>) -> Self {
        MockDb {
            responses: responses.into(),
            calls: Vec::new(),
            tx: Vec::new(),
            fail: None,
        }
    }

    /// A mock whose every `fetch`/`execute` fails with `message` (the DB-fault path).
    pub fn failing(message: impl Into<String>) -> Self {
        MockDb {
            fail: Some(message.into()),
            ..MockDb::default()
        }
    }
}

impl Db for MockDb {
    fn fetch(&mut self, sql: &str, params: &[SqlValue]) -> Result<Vec<Row>, DbError> {
        self.calls.push((sql.to_string(), params.to_vec()));
        if let Some(m) = &self.fail {
            return Err(DbError::new(m.clone()));
        }
        Ok(self.responses.pop_front().unwrap_or_default())
    }

    fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.calls.push((sql.to_string(), params.to_vec()));
        if let Some(m) = &self.fail {
            return Err(DbError::new(m.clone()));
        }
        Ok(0)
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.tx.push("begin");
        Ok(())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        self.tx.push("commit");
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        self.tx.push("rollback");
        Ok(())
    }
}
