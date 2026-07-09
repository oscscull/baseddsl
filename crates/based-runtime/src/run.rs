//! Executing a planned query and shaping the rows into the response envelope.
//!
//! Execution goes through the abstract [`Db`] trait — the runtime's twin of the
//! generated client's abstract `Transport`; concrete drivers (`sqlite`, `driver`,
//! `postgres`) implement it, and a [`MockDb`] returns canned rows so the whole
//! request → JSON path is testable with no database. Row shaping is where the
//! envelope becomes real: `get` → a JSON object or `null`, `list` → an array, a
//! paginated `list` → the `{ rows, cursor }` page envelope (the keyset cursor is
//! minted here from the last row's hidden sort-key columns).

use crate::id::IdGen;
use crate::idempotency::{IdempotencyStore, KeyState};
use crate::load::Compiled;
use crate::plan::{
    plan_mutation, plan_query, Envelope, KeysetPlan, MutationPlan, PlanError, QueryPlan, Request,
    Stmt,
};
use crate::value::SqlValue;
use based_codegen::sql::{ARRAY_MARK, KEYSET_PREFIX};

/// One returned row: column alias → JSON value (the SELECT aliases each projection
/// to its output name, so a row is already the response object).
pub type Row = serde_json::Map<String, serde_json::Value>;

/// A failure from the database itself — connection lost, timeout, deadlock, a shard
/// down, pool exhausted. Distinct from a [`PlanError`] (a boundary/validation failure
/// *before* any SQL): a `DbError` is an operational failure the wire maps to a
/// retryable `503`. The message is human-facing; the driver fills it from its error.
///
/// The [`kind`](DbError::kind) is the driver's classification of how to handle the failure:
/// every `DbError` is still a `503`, but a [`Deadlock`](DbErrorKind::Deadlock) additionally
/// tells the mutation path the transaction is safe to auto-retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    pub message: String,
    pub kind: DbErrorKind,
}

/// The operational class of a [`DbError`], set by the driver from the server's error code.
/// Only [`Deadlock`](DbErrorKind::Deadlock) changes engine behaviour (bounded transaction
/// retry); the rest are informational — every kind is still a wire `503`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DbErrorKind {
    /// An unclassified operational failure (connection lost, a statement timeout, a
    /// constraint violation). A `503` the caller may retry, but the engine does **not**
    /// auto-retry — re-running a statement timeout or a lost connection just fails again.
    #[default]
    Other,
    /// A deadlock or serialization failure: the server *already rolled the transaction
    /// back*, and re-running it usually succeeds (the contending transaction has moved
    /// on). The mutation path retries the whole transaction a bounded number of times.
    /// MariaDB 1213/1205, Postgres 40P01/40001, SQLite `SQLITE_BUSY`/`SQLITE_LOCKED`.
    Deadlock,
    /// No connection became free within the pool's checkout timeout — the pool is
    /// saturated. Fails fast as a `503` (the client/LB backs off), never a hang and never
    /// auto-retried in-process (the pool is still full).
    PoolExhausted,
}

impl DbError {
    /// An unclassified ([`Other`](DbErrorKind::Other)) operational failure.
    pub fn new(message: impl Into<String>) -> DbError {
        DbError {
            message: message.into(),
            kind: DbErrorKind::Other,
        }
    }

    /// A failure of a specific operational [`DbErrorKind`] (the driver classifies its own
    /// error codes into these).
    pub fn of(kind: DbErrorKind, message: impl Into<String>) -> DbError {
        DbError {
            message: message.into(),
            kind,
        }
    }

    /// Is this a deadlock / serialization abort the mutation path may safely retry?
    pub fn is_deadlock(&self) -> bool {
        self.kind == DbErrorKind::Deadlock
    }

    /// A stable machine-readable code for the operational class of this failure.
    pub fn code(&self) -> &'static str {
        match self.kind {
            DbErrorKind::Other => "database_error",
            DbErrorKind::Deadlock => "deadlock",
            DbErrorKind::PoolExhausted => "pool_exhausted",
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DbError {}

/// Why running a request failed: a boundary [`PlanError`] (bad/missing input, unknown
/// callable — the caller can fix it), a [`DbError`] (the database failed — an
/// operational, retryable failure), or an idempotency [`Conflict`](RunError::Conflict)
/// (a concurrent attempt with the same key is still in flight). The wire maps each to its
/// HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Plan(PlanError),
    Db(DbError),
    /// A mutation retry arrived while a prior attempt with the same idempotency key is
    /// still running. Running a second write would risk the double-insert the key exists to
    /// prevent, so the retry is rejected as a retryable conflict (`409`): the client retries
    /// once the first attempt settles.
    Conflict(String),
    /// The idempotency key was reused for a different request — same key, different
    /// args/`$ctx`. Replaying the first attempt's response would answer the wrong request,
    /// so the reuse is rejected loudly (a non-retryable `422`) rather than run or replayed.
    /// The client must use a fresh key for a genuinely different request.
    KeyReuse(String),
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

impl RunError {
    /// A stable machine-readable code for the failure — the boundary/operational class a
    /// consumer branches on. Delegates to the inner [`PlanError::code`]/[`DbError::code`]
    /// where the failure carries its own; the idempotency variants own theirs.
    pub fn code(&self) -> &'static str {
        match self {
            RunError::Plan(e) => e.code(),
            RunError::Db(e) => e.code(),
            RunError::Conflict(_) => "idempotency_conflict",
            RunError::KeyReuse(_) => "idempotency_key_reuse",
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Plan(e) => write!(f, "{e}"),
            RunError::Db(e) => write!(f, "{e}"),
            RunError::Conflict(key) => {
                write!(
                    f,
                    "a request with idempotency key `{key}` is already in progress"
                )
            }
            RunError::KeyReuse(key) => write!(
                f,
                "idempotency key `{key}` was already used for a different request"
            ),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Plan(e) => Some(e),
            RunError::Db(e) => Some(e),
            RunError::Conflict(_) | RunError::KeyReuse(_) => None,
        }
    }
}

/// The database seam. The runtime hands it positional SQL + values; the read path
/// `fetch`es rows, the write path `execute`s statements under an engine-owned
/// transaction (the engine, not the emitted SQL, owns BEGIN/COMMIT). Every method is
/// fallible: a dependable driver surfaces connection/query failures rather than
/// panicking. The write methods default so a read-only [`Db`] need not implement them.
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

/// A source of per-request database connections for the listener, keyed by shard.
/// Given a request's shard key it hands back a boxed [`Db`] to run that request on
/// (single-shard dispatch). This is the seam that keeps the HTTP edge driver-neutral:
/// the MariaDB [`crate::driver::ShardRouter`] is one implementation; a Postgres / MySQL /
/// SQLite backend is another (the [`Db`] trait below is already dialect-agnostic — it
/// speaks positional SQL + [`SqlValue`], not a MariaDB protocol). A single-file SQLite
/// backend simply ignores the key and returns the one connection.
pub trait Backend: Send + Sync {
    /// Check out a connection for the shard the key routes to. A failure (pool
    /// exhausted, shard/host down) is a [`DbError`] → the wire's retryable `503`.
    fn checkout(&self, shard_key: &str) -> Result<Box<dyn Db>, DbError>;

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
    fn ping(&self) -> Result<(), DbError> {
        self.checkout("").map(|_| ())
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
///
/// When the request carries an idempotency key the write body runs at most once per
/// `(callable, key)`: a first attempt claims the key, runs, and records its response; a
/// retry replays that recorded response with no writes (exactly-once), and a concurrent
/// retry while the first is still in flight is a [`RunError::Conflict`]. Planning (arg /
/// `$ctx` validation) happens before the store is consulted, so a malformed request is a
/// clean `4xx` that never claims a key. Without a key this is the plain run-every-time path.
pub fn run_mutation(
    compiled: &Compiled,
    db: &mut dyn Db,
    id_gen: &mut dyn IdGen,
    store: &dyn IdempotencyStore,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    // Plan first: a bad arg / missing `$ctx` is a boundary error that must not consume an
    // idempotency slot (a client fixes the request and retries with the *same* key).
    let plan = plan_mutation(compiled, req, id_gen)?;

    // No key → the plain path (run every time). This is also what `NoStore` yields, but
    // short-circuiting here means a keyless request never touches the store at all.
    let key = match &req.idempotency_key {
        None => return Ok(apply(db, &plan)?),
        Some(k) => k,
    };

    // Fingerprint the request payload (args + `$ctx`) so the store can tell a genuine
    // retry (same payload) from one key reused for a different request.
    match store.begin(&req.callable, key, req.fingerprint()) {
        // A prior attempt with the same payload already committed: replay it, run no writes.
        KeyState::Done(response) => Ok(response),
        // A concurrent attempt (same payload) is still running: don't run a second write.
        KeyState::InFlight => Err(RunError::Conflict(key.clone())),
        // Same key, *different* payload: reject — replaying would answer the wrong request.
        KeyState::Mismatch => Err(RunError::KeyReuse(key.clone())),
        // Fresh: we hold the claim. Run the write, then record its response — or release
        // the claim on failure so a later retry (same key) may try again.
        KeyState::Fresh => match apply(db, &plan) {
            Ok(response) => {
                store.record(&req.callable, key, response.clone());
                Ok(response)
            }
            Err(e) => {
                store.abandon(&req.callable, key);
                Err(e.into())
            }
        },
    }
}

/// How many times the mutation path re-runs a transaction the server aborted for a
/// deadlock / serialization conflict before giving up. Bounded so a pathological hot row
/// fails fast as a `503` rather than retrying forever; a handful of attempts clears an
/// ordinary two-transaction deadlock (the loser re-runs after the winner commits). Total
/// attempts = 1 + this.
const TX_RETRY_LIMIT: u32 = 5;

/// Backoff before re-running a deadlocked transaction: a short exponential step (capped
/// at 100ms — a deadlock clears in milliseconds once the winner commits) plus jitter, so
/// two transactions that just deadlocked don't retry in lockstep and collide again.
fn deadlock_backoff(attempt: u32) -> std::time::Duration {
    let step_ms = 2u64.saturating_pow(attempt).saturating_mul(2).min(100);
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % step_ms.max(1))
        .unwrap_or(0);
    std::time::Duration::from_millis(step_ms + jitter)
}

/// Execute a mutation's transaction, retrying the whole thing on a deadlock. A
/// deadlock/serialization abort ([`DbErrorKind::Deadlock`]) rolled the transaction back
/// server-side, so re-running it from the top on the same connection usually succeeds once
/// the contending transaction commits; a bounded [`TX_RETRY_LIMIT`] then a `503` prevents a
/// hot row retrying forever. Every other failure surfaces immediately (no auto-retry).
fn apply(db: &mut dyn Db, plan: &MutationPlan) -> Result<serde_json::Value, DbError> {
    let mut attempt = 0u32;
    loop {
        match apply_once(db, plan) {
            Err(e) if e.is_deadlock() && attempt < TX_RETRY_LIMIT => {
                attempt += 1;
                std::thread::sleep(deadlock_backoff(attempt));
                // The server already rolled the aborted transaction back; re-run it.
            }
            result => return result,
        }
    }
}

/// Run a mutation plan's writes in order under one transaction, then assemble the write
/// response. If any write fails the transaction is rolled back and the error surfaced — a
/// mutation is all-or-nothing, never a partial write. Wrapped by [`apply`] for the
/// deadlock-retry loop.
///
/// The response is the written row read back in the mutation's declared shape: when the
/// plan carries a re-select, it runs inside the same transaction (read-your-writes, atomic
/// with the writes) and its single row is the response — matching the client's decoded
/// output type. Only a mutation whose row does not survive the write (a real DELETE) has no
/// re-select and falls back to `{ id }` / `{}`.
fn apply_once(db: &mut dyn Db, plan: &MutationPlan) -> Result<serde_json::Value, DbError> {
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
    // Read the written row back in its declared shape, still inside the transaction.
    let response = match &plan.ret_select {
        Some(stmt) => match db.fetch(&stmt.sql, &stmt.params) {
            Ok(rows) => rows.into_iter().next().map(nest_row).unwrap_or(J::Null),
            Err(e) => {
                let _ = db.rollback();
                return Err(e);
            }
        },
        // No declared-shape re-select (the row did not survive — a real DELETE): identify
        // the created row by its engine `id`, or an empty object when nothing was created.
        None => match &plan.result_id {
            Some(id) => {
                let mut obj = serde_json::Map::new();
                obj.insert("id".into(), J::String(id.clone()));
                J::Object(obj)
            }
            None => J::Object(serde_json::Map::new()),
        },
    };
    db.commit()?;
    Ok(response)
}

/// Reassemble a flat result row into the response object, nesting sub-objects/arrays.
///
/// A nested to-one shape sub-object (`buyer { name, email }`) is projected by codegen
/// as columns aliased `buyer.name`, `buyer.email` ([`based_codegen::sql::NEST_SEP`] is
/// the `.`); this splits each such key back into a nested object, recursing for
/// nested-within-nested (`buyer.org.name`). A to-many nest (`items { … }`) is projected as
/// a single JSON-array string column aliased `items[]`
/// ([`based_codegen::sql::ARRAY_MARK`]); this parses the string into a real JSON array of
/// sub-objects (their own nesting already fully formed by the SQL JSON aggregation). A
/// `.`/`[`/`]` cannot occur in a BSL identifier, so a flat query (no nest) has no such key
/// and passes through unchanged.
fn nest_row(row: Row) -> serde_json::Value {
    let mut root = serde_json::Map::new();
    for (key, val) in row {
        insert_path(&mut root, &key, val);
    }
    serde_json::Value::Object(root)
}

/// Parse a to-many array column's value into a JSON array. The DB returns the aggregated
/// column as a JSON-array *string* (SQLite/MariaDB text); a driver that decodes the JSON
/// type natively hands back an array already, and an empty group may arrive as NULL — all
/// three normalize to an array here (a malformed string, which the engine never emits,
/// degrades to `[]` rather than panicking).
fn parse_array(val: serde_json::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match val {
        J::String(s) => serde_json::from_str(&s).unwrap_or(J::Array(Vec::new())),
        arr @ J::Array(_) => arr,
        _ => J::Array(Vec::new()),
    }
}

/// Insert `val` at a possibly-dotted `key` into `obj`, creating intermediate objects for
/// each `NEST_SEP` segment (`buyer.org.name` → `{buyer:{org:{name:val}}}`). A leaf key
/// suffixed with `ARRAY_MARK` (`items[]`) is a to-many array: its string value is parsed
/// into a JSON array and stored under the field name without the marker.
fn insert_path(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    val: serde_json::Value,
) {
    match key.split_once(based_codegen::sql::NEST_SEP) {
        None => match key.strip_suffix(ARRAY_MARK) {
            Some(name) => {
                obj.insert(name.to_string(), parse_array(val));
            }
            None => {
                obj.insert(key.to_string(), val);
            }
        },
        Some((head, rest)) => {
            let entry = obj
                .entry(head.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(child) = entry {
                insert_path(child, rest, val);
            }
        }
    }
}

/// Mint the "more" cursor for a keyset page: the last row's sort-key values, read from the
/// hidden `__keyset_<i>` columns codegen projected. Only a full page (`page_size` rows) can
/// have a next page — a short page is the last, so it gets no cursor (the caller stops
/// paging rather than making one more empty request).
fn next_cursor(rows: &[Row], ks: KeysetPlan) -> Option<String> {
    use serde_json::Value as J;
    if (rows.len() as u64) < ks.page_size {
        return None;
    }
    let last = rows.last()?;
    let vals: Vec<J> = (0..ks.keys)
        .map(|i| {
            last.get(&format!("{KEYSET_PREFIX}{i}"))
                .cloned()
                .unwrap_or(J::Null)
        })
        .collect();
    Some(crate::cursor::encode(&vals))
}

/// Execute a plan's statements and assemble the response per its envelope.
fn shape(db: &mut dyn Db, plan: &QueryPlan) -> Result<serde_json::Value, DbError> {
    use serde_json::Value as J;
    let mut rows = run_stmt(db, &plan.main)?;
    Ok(match plan.envelope {
        // `get`: the first row, or JSON null (Option<T>).
        Envelope::One => rows.into_iter().next().map(nest_row).unwrap_or(J::Null),
        // `list`: every row as an array.
        Envelope::Many => J::Array(rows.into_iter().map(nest_row).collect()),
        // paginated `list`: the { rows, cursor } envelope. For a keyset page, mint the
        // next cursor from the last row's hidden sort-key columns and strip them from
        // the response; `total` rides along when the query asked for a count.
        Envelope::Page { with_count } => {
            let cursor = plan.keyset.and_then(|ks| next_cursor(&rows, ks));
            if plan.keyset.is_some() {
                for r in &mut rows {
                    r.retain(|k, _| !k.starts_with(KEYSET_PREFIX));
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert(
                "rows".into(),
                J::Array(rows.into_iter().map(nest_row).collect()),
            );
            obj.insert("cursor".into(), cursor.map(J::String).unwrap_or(J::Null));
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
