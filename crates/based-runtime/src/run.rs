//! Executing a planned query and shaping the rows into the response envelope.
//!
//! Execution goes through the abstract [`DbRead`]/[`Db`]/[`Tx`]/[`Backend`] traits —
//! the runtime's twin of the generated client's abstract `Transport`; concrete drivers
//! (`sqlite`, `driver`, `postgres`) implement them, and a [`MockDb`] returns canned
//! rows so the whole request → JSON path is testable with no database. Row shaping is
//! where the envelope becomes real: `get` → a JSON object or `null`, `list` → an
//! array, a paginated `list` → the `{ rows, cursor }` page envelope (the keyset cursor
//! is minted here from the last row's hidden sort-key columns).
//!
//! Reads have exactly one path: [`DbRead::fetch`] returns a fallible row *stream*,
//! always — a one-shot response is a collect at this layer, and a streaming wire
//! surface consumes the same stream. Transactions are a consuming typestate:
//! [`Db::begin`] takes the connection, [`Tx::commit`] takes the transaction, and a
//! `Tx` dropped without commit rolls back or discards its connection — an open
//! transaction can never re-enter the pool, and a cancelled caller can never leave a
//! half-written mutation behind.

use async_trait::async_trait;

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

/// The one read shape: a fallible stream of rows borrowed from the connection it
/// runs on. A one-shot caller collects it ([`fetch_all`]); a streaming caller
/// consumes it row by row.
pub type RowStream<'a> = futures_core::stream::BoxStream<'a, Result<Row, DbError>>;

/// Collect a [`RowStream`] into a `Vec` — the one-shot read path.
pub async fn fetch_all(stream: RowStream<'_>) -> Result<Vec<Row>, DbError> {
    use futures_util::TryStreamExt;
    stream.try_collect().await
}

/// An owned stream of shaped response rows — a `-> stream` query's payload. Each item
/// is exactly one element of what the `[]` form's array would be (nests materialized
/// within the row), in sort order. The stream owns the connection it reads on;
/// dropping it mid-pass cancels the read and returns the connection to the pool
/// (reads hold no transaction). After an `Err` item the stream is finished.
pub type ShapedStream =
    futures_core::stream::BoxStream<'static, Result<serde_json::Value, DbError>>;

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
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: DbErrorKind::Other,
        }
    }

    /// A failure of a specific operational [`DbErrorKind`] (the driver classifies its own
    /// error codes into these).
    pub fn of(kind: DbErrorKind, message: impl Into<String>) -> Self {
        Self {
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
/// operational, retryable failure), a [`NotFound`](RunError::NotFound) (the mutation's
/// `where` matched no row), or an idempotency [`Conflict`](RunError::Conflict)
/// (a concurrent attempt with the same key is still in flight). The wire maps each to its
/// HTTP status.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Plan(PlanError),
    Db(DbError),
    /// A surviving-write mutation (update / soft delete / restore) matched no row: its
    /// `where` — with the scope and soft-delete guards it carries — found nothing to
    /// write, so nothing was written and there is no row to read back. Surfaced as a
    /// `404` rather than a `200 null` the typed client cannot decode. Carries the
    /// callable name.
    NotFound(String),
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
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}
impl From<DbError> for RunError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

impl RunError {
    /// A stable machine-readable code for the failure — the boundary/operational class a
    /// consumer branches on. Delegates to the inner [`PlanError::code`]/[`DbError::code`]
    /// where the failure carries its own; the idempotency variants own theirs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Plan(e) => e.code(),
            Self::Db(e) => e.code(),
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "idempotency_conflict",
            Self::KeyReuse(_) => "idempotency_key_reuse",
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plan(e) => write!(f, "{e}"),
            Self::Db(e) => write!(f, "{e}"),
            Self::NotFound(name) => write!(
                f,
                "`{name}` matched no row (no such row, or it is out of scope)"
            ),
            Self::Conflict(key) => {
                write!(
                    f,
                    "a request with idempotency key `{key}` is already in progress"
                )
            }
            Self::KeyReuse(key) => write!(
                f,
                "idempotency key `{key}` was already used for a different request"
            ),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(e) => Some(e),
            Self::Db(e) => Some(e),
            Self::NotFound(_) | Self::Conflict(_) | Self::KeyReuse(_) => None,
        }
    }
}

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
    /// Open the transaction a mutation body runs in, consuming the connection.
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError>;
}

/// An open transaction. [`commit`](Tx::commit) consumes it; dropping it without
/// commit rolls back or discards the connection (never pooled with an open tx), so a
/// write can only survive via `commit` — cancellation at any await point cannot
/// double-write.
#[async_trait]
pub trait Tx: DbRead {
    async fn commit(self: Box<Self>) -> Result<(), DbError>;
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

/// Plan and run a query request, returning the shaped JSON response. Takes any
/// [`DbRead`] — a checked-out connection or an open transaction (generic so a
/// `&mut dyn Db` / `&mut dyn Tx` passes straight in).
pub async fn run_query<D: DbRead + ?Sized>(
    compiled: &Compiled,
    db: &mut D,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    let plan = plan_query(compiled, req)?;
    Ok(shape(db, &plan).await?)
}

/// Plan a query request and return its rows as an owned [`ShapedStream`] — the
/// `-> stream` read path. Planning (arg / `$ctx` validation) happens before the first
/// row, so a boundary failure is an ordinary [`PlanError`] and the stream never
/// starts. The same plan → fetch → shape path as [`run_query`], minus the collect:
/// scope, soft-delete, and shaping are identical to the `[]` form.
///
/// Takes the connection by value: the returned stream owns it for the whole pass, and
/// dropping the stream (caller cancelled) drops the connection back to the pool.
pub fn run_query_stream(
    compiled: &Compiled,
    mut db: Box<dyn Db>,
    req: &Request,
) -> Result<ShapedStream, PlanError> {
    use futures_util::StreamExt;
    let plan = plan_query(compiled, req)?;
    Ok(Box::pin(async_stream::stream! {
        let mut rows = db.fetch(&plan.main.sql, &plan.main.params);
        while let Some(item) = rows.next().await {
            match item {
                Ok(row) => yield Ok(nest_row(row)),
                // A mid-stream failure is the stream's last item.
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }
    }))
}

/// Plan and run a mutation request: id-gen + bind, then execute every write under one
/// engine-owned transaction, returning the write response. Takes the [`Backend`]
/// (not a connection): each transaction attempt — including a deadlock re-run — is a
/// fresh checkout + fresh [`Tx`], so a failed attempt's connection is already back in
/// the pool (or discarded) before the next begins.
///
/// When the request carries an idempotency key the write body runs at most once per
/// `(callable, key)`: a first attempt claims the key, runs, and records its response; a
/// retry replays that recorded response with no writes (exactly-once), and a concurrent
/// retry while the first is still in flight is a [`RunError::Conflict`]. Planning (arg /
/// `$ctx` validation) happens before the store is consulted, so a malformed request is a
/// clean `4xx` that never claims a key. Without a key this is the plain run-every-time path.
pub async fn run_mutation(
    compiled: &Compiled,
    backend: &dyn Backend,
    shard_key: &str,
    id_gen: &dyn IdGen,
    store: &dyn IdempotencyStore,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    // Plan first: a bad arg / missing `$ctx` is a boundary error that must not consume an
    // idempotency slot (a client fixes the request and retries with the *same* key).
    let plan = plan_mutation(compiled, req, id_gen)?;

    // No key → the plain path (run every time). This is also what `NoStore` yields, but
    // short-circuiting here means a keyless request never touches the store at all.
    let Some(key) = &req.idempotency_key else {
        return apply(backend, shard_key, &plan)
            .await?
            .ok_or_else(|| RunError::NotFound(req.callable.clone()));
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
        // Fresh: we hold the claim. Run the write, then record its response. The guard
        // releases the claim on any exit that records nothing — a write failure, a
        // not-found (nothing was written, so a retry may run once the row exists), or
        // the caller dropping this future mid-write (cancellation) — so a later retry
        // (same key) may try again instead of hitting a stranded in-flight claim forever.
        KeyState::Fresh => {
            let mut claim = Claim {
                store,
                callable: &req.callable,
                key,
                armed: true,
            };
            let response = apply(backend, shard_key, &plan)
                .await?
                .ok_or_else(|| RunError::NotFound(req.callable.clone()))?;
            claim.armed = false;
            store.record(&req.callable, key, response.clone());
            Ok(response)
        }
    }
}

/// An armed idempotency claim: dropped without being disarmed (write failure, or the
/// mutation future cancelled at an await point), it releases the key so a retry may run.
/// A drop while the commit itself is in flight has an unknown outcome; releasing there
/// matches the existing failed-commit semantics — a durable store that resolves the
/// claim atomically with the transaction is the deferred multi-instance answer.
struct Claim<'a> {
    store: &'a dyn IdempotencyStore,
    callable: &'a str,
    key: &'a str,
    armed: bool,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.store.abandon(self.callable, self.key);
        }
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
        .map_or(0, |d| u64::from(d.subsec_nanos()) % step_ms.max(1));
    std::time::Duration::from_millis(step_ms + jitter)
}

/// Execute a mutation's transaction, retrying the whole thing on a deadlock. A
/// deadlock/serialization abort ([`DbErrorKind::Deadlock`]) rolled the transaction back
/// server-side; each retry is a fresh checkout + fresh [`Tx`], so re-running usually
/// succeeds once the contending transaction commits. A bounded [`TX_RETRY_LIMIT`] then a
/// `503` prevents a hot row retrying forever. Every other failure surfaces immediately.
/// `Ok(None)` is [`apply_once`]'s matched-no-row outcome, passed through.
async fn apply(
    backend: &dyn Backend,
    shard_key: &str,
    plan: &MutationPlan,
) -> Result<Option<serde_json::Value>, DbError> {
    let mut attempt = 0u32;
    loop {
        let db = backend.checkout(shard_key).await?;
        match apply_once(db, plan).await {
            Err(e) if e.is_deadlock() && attempt < TX_RETRY_LIMIT => {
                attempt += 1;
                tokio::time::sleep(deadlock_backoff(attempt)).await;
                // The server already rolled the aborted transaction back; re-run it.
            }
            result => return result,
        }
    }
}

/// Run a mutation plan's writes in order under one transaction, then assemble the write
/// response. A failed write (or a caller cancelling mid-body) drops the [`Tx`], which
/// rolls back — a mutation is all-or-nothing, never a partial write. Wrapped by
/// [`apply`] for the deadlock-retry loop.
///
/// The response is the written row read back in the mutation's declared shape: when the
/// plan carries a re-select, it runs inside the same transaction (read-your-writes, atomic
/// with the writes) and its single row is the response — matching the client's decoded
/// output type. A re-select that finds **no row** means the write's `where` (with its
/// scope/soft-delete guards) matched nothing: the transaction is dropped (rollback, so a
/// sibling write in the same body never survives the miss) and `Ok(None)` reports the
/// not-found. Only a mutation whose row does not survive the write (a real DELETE) has no
/// re-select and falls back to `{ id }` / `{}`.
///
/// An `-> ok` mutation (a real DELETE, no re-select) decides the miss on rows
/// affected instead: its primary DELETE (`plan.ack_check`) touching zero rows means
/// the row was absent or out of scope — same rollback, same `Ok(None)` not-found.
async fn apply_once(
    db: Box<dyn Db>,
    plan: &MutationPlan,
) -> Result<Option<serde_json::Value>, DbError> {
    use serde_json::Value as J;
    let mut tx = db.begin().await?;
    for (i, stmt) in plan.stmts.iter().enumerate() {
        // An error propagates and drops `tx` → rollback (never a pooled open tx).
        let affected = tx.execute(&stmt.sql, &stmt.params).await?;
        if plan.ack_check == Some(i) && affected == 0 {
            return Ok(None);
        }
    }
    // Read the written row back in its declared shape, still inside the transaction.
    let response = match &plan.ret_select {
        Some(stmt) => {
            let rows = fetch_all(tx.fetch(&stmt.sql, &stmt.params)).await?;
            match rows.into_iter().next() {
                Some(row) => nest_row(row),
                None => return Ok(None),
            }
        }
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
    tx.commit().await?;
    Ok(Some(response))
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
    let mut value = serde_json::Value::Object(root);
    collapse_absent_nests(&mut value);
    value
}

/// Collapse absent to-one nests. A LEFT-JOINed nest projects a presence probe
/// (`<field>.__present` = the child's `id`, [`based_codegen::sql::NEST_PRESENT`]):
/// a NULL probe means the joined row does not exist, so the whole sub-object —
/// otherwise an indistinguishable object of NULLs — becomes JSON null. A matched
/// row just sheds the probe. Recurses for nests within nests.
fn collapse_absent_nests(value: &mut serde_json::Value) {
    use serde_json::Value as J;
    if let J::Object(map) = value {
        if let Some(probe) = map.remove(based_codegen::sql::NEST_PRESENT) {
            if probe.is_null() {
                *value = J::Null;
                return;
            }
        }
        for child in map.values_mut() {
            collapse_absent_nests(child);
        }
    }
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
async fn shape<D: DbRead + ?Sized>(
    db: &mut D,
    plan: &QueryPlan,
) -> Result<serde_json::Value, DbError> {
    use serde_json::Value as J;
    let mut rows = fetch_all(db.fetch(&plan.main.sql, &plan.main.params)).await?;
    Ok(match plan.envelope {
        // `get`: the first row, or JSON null (Option<T>).
        Envelope::One => rows.into_iter().next().map_or(J::Null, nest_row),
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
            obj.insert("cursor".into(), cursor.map_or(J::Null, J::String));
            if with_count {
                if let Some(count) = &plan.count {
                    let total = fetch_all(db.fetch(&count.sql, &count.params))
                        .await?
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

/// Run one statement to completion — the collected one-shot read, for callers holding
/// a [`Stmt`].
pub async fn run_stmt<D: DbRead + ?Sized>(db: &mut D, stmt: &Stmt) -> Result<Vec<Row>, DbError> {
    fetch_all(db.fetch(&stmt.sql, &stmt.params)).await
}

// ---------- the mock -------------------------------------------------------

#[derive(Default)]
struct MockState {
    responses: std::collections::VecDeque<Vec<Row>>,
    calls: Vec<(String, Vec<SqlValue>)>,
    tx: Vec<&'static str>,
    fail: Option<String>,
    /// `fetch` yields its batch, then this failure — the stream-broke-late case.
    fail_mid_stream: Option<String>,
    /// What every `execute` reports as rows affected (default 0).
    affected: u64,
}

/// A test double for the whole driver stack: it is a [`Backend`] (checkout clones the
/// shared state), a [`Db`], and — via [`Db::begin`] — a [`Tx`]. It returns pre-loaded
/// row batches in call order, recording every `(sql, params)` it was asked to run
/// (`fetch` and `execute` alike) plus the transaction boundaries it saw, so tests can
/// assert the bound statements. Cheap to clone; every clone shares the same state, so
/// a test keeps a handle for assertions while the engine consumes another.
#[derive(Clone, Default)]
pub struct MockDb {
    state: std::sync::Arc<std::sync::Mutex<MockState>>,
}

impl MockDb {
    /// A mock that replies to each `fetch` with the given batches, in order.
    pub fn new(responses: Vec<Vec<Row>>) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MockState {
                responses: responses.into(),
                ..MockState::default()
            })),
        }
    }

    /// A mock whose every `fetch`/`execute` fails with `message` (the DB-fault path).
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MockState {
                fail: Some(message.into()),
                ..MockState::default()
            })),
        }
    }

    /// A mock whose `fetch` yields `rows`, then fails with `message` — the database
    /// breaking *mid-stream*, after the read has started delivering.
    pub fn failing_mid_stream(rows: Vec<Row>, message: impl Into<String>) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MockState {
                responses: vec![rows].into(),
                fail_mid_stream: Some(message.into()),
                ..MockState::default()
            })),
        }
    }

    /// Report `rows` as every `execute`'s rows-affected (default 0) — the knob the
    /// `-> ok` zero-row-DELETE tests turn.
    pub fn affecting(self, rows: u64) -> Self {
        self.state.lock().unwrap().affected = rows;
        self
    }

    /// Every executed statement so far, in order — `fetch` and `execute` alike.
    pub fn calls(&self) -> Vec<(String, Vec<SqlValue>)> {
        self.state.lock().unwrap().calls.clone()
    }

    /// The transaction boundaries seen, in order (`begin`/`commit`/`rollback` — a
    /// dropped-without-commit [`Tx`] records `rollback`).
    pub fn tx_log(&self) -> Vec<&'static str> {
        self.state.lock().unwrap().tx.clone()
    }

    fn record(&self, sql: &str, params: &[SqlValue]) -> Result<(), DbError> {
        let mut st = self.state.lock().unwrap();
        st.calls.push((sql.to_string(), params.to_vec()));
        match &st.fail {
            Some(m) => Err(DbError::new(m.clone())),
            None => Ok(()),
        }
    }

    fn pop(&self) -> Vec<Row> {
        self.state
            .lock()
            .unwrap()
            .responses
            .pop_front()
            .unwrap_or_default()
    }
}

#[async_trait]
impl DbRead for MockDb {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        let items: Vec<Result<Row, DbError>> = match self.record(sql, params) {
            Ok(()) => {
                let mut items: Vec<Result<Row, DbError>> = self.pop().into_iter().map(Ok).collect();
                if let Some(m) = self.state.lock().unwrap().fail_mid_stream.clone() {
                    items.push(Err(DbError::new(m)));
                }
                items
            }
            Err(e) => vec![Err(e)],
        };
        Box::pin(futures_util::stream::iter(items))
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.record(sql, params)?;
        Ok(self.state.lock().unwrap().affected)
    }
}

#[async_trait]
impl Db for MockDb {
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError> {
        self.state.lock().unwrap().tx.push("begin");
        Ok(Box::new(MockTx {
            db: *self,
            committed: false,
        }))
    }
}

#[async_trait]
impl Backend for MockDb {
    async fn checkout(&self, _shard_key: &str) -> Result<Box<dyn Db>, DbError> {
        Ok(Box::new(self.clone()))
    }

    /// A mock has no live database — trivially ready.
    async fn ping(&self) -> Result<(), DbError> {
        Ok(())
    }
}

/// The mock's open transaction: statements delegate to the shared state; drop without
/// commit records the rollback the typestate guarantees.
struct MockTx {
    db: MockDb,
    committed: bool,
}

#[async_trait]
impl DbRead for MockTx {
    fn fetch<'a>(&'a mut self, sql: &'a str, params: &[SqlValue]) -> RowStream<'a> {
        self.db.fetch(sql, params)
    }

    async fn execute(&mut self, sql: &str, params: &[SqlValue]) -> Result<u64, DbError> {
        self.db.execute(sql, params).await
    }
}

#[async_trait]
impl Tx for MockTx {
    async fn commit(mut self: Box<Self>) -> Result<(), DbError> {
        self.committed = true;
        self.db.state.lock().unwrap().tx.push("commit");
        Ok(())
    }
}

impl Drop for MockTx {
    fn drop(&mut self) {
        if !self.committed {
            self.db.state.lock().unwrap().tx.push("rollback");
        }
    }
}
