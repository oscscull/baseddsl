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
use crate::idempotency::{Fingerprint, IdempotencyStore, KeyState, TxClaim, TxIdempotency};
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
    /// Open the transaction a mutation body runs in, consuming the connection. Uses the
    /// driver's default isolation — the auto-committing mutation path takes this.
    async fn begin(self: Box<Self>) -> Result<Box<dyn Tx>, DbError>;

    /// Open a transaction at the requested isolation + access mode — the explicit
    /// read-decide-write seam ([`crate::Engine::begin`]). A driver applies the per-dialect
    /// isolation SQL ([`based_codegen::Dialect::begin_transaction_sql`]); the default
    /// ignores `opts` (the mock, and any driver with no isolation control) and opens a
    /// plain transaction.
    async fn begin_tx(self: Box<Self>, opts: crate::tx::TxOptions) -> Result<Box<dyn Tx>, DbError> {
        let _ = opts;
        self.begin().await
    }
}

/// An open transaction. [`commit`](Tx::commit) consumes it; dropping it without
/// commit rolls back or discards the connection (never pooled with an open tx), so a
/// write can only survive via `commit` — cancellation at any await point cannot
/// double-write.
#[async_trait]
pub trait Tx: DbRead {
    async fn commit(self: Box<Self>) -> Result<(), DbError>;

    /// Roll the transaction back explicitly, consuming it. The default drops `self` — the
    /// same rollback the typestate already guarantees on drop; a driver with an awaitable
    /// rollback (all three real drivers) overrides this so the rollback completes before
    /// the connection returns to the pool.
    async fn rollback(self: Box<Self>) -> Result<(), DbError> {
        Ok(())
    }
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
                Ok(row) => {
                    let mut v = nest_row(row);
                    if !plan.json_paths.is_empty() {
                        normalize_json(&mut v, &plan.json_paths);
                    }
                    yield Ok(v);
                }
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
    let key = req.idempotency_key.as_ref();

    // A tx-participant store (the durable DB-backed one) commits the key *inside* the
    // mutation's own transaction, so it can't bracket the write from out here: hand the
    // claim context to `apply`, which claims right after `begin` and records right before
    // `commit`. Concurrency is block-and-replay (a concurrent retry blocks on the key's
    // unique index, then replays), so there is no in-flight/409 branch for this store.
    if let (Some(key), Some(participant)) = (key, store.tx_participant()) {
        let claim = TxClaimCtx {
            participant,
            callable: &req.callable,
            key,
            fingerprint: req.fingerprint(),
        };
        return match apply(backend, shard_key, &plan, Some(&claim)).await? {
            TxOutcome::Done(r) | TxOutcome::Replayed(r) => Ok(r),
            TxOutcome::Mismatch => Err(RunError::KeyReuse(key.clone())),
            TxOutcome::NotFound => Err(RunError::NotFound(req.callable.clone())),
        };
    }

    // No key → the plain path (run every time). This is also what `NoStore` yields, but
    // short-circuiting here means a keyless request never touches the store at all.
    let Some(key) = key else {
        return plain_outcome(apply(backend, shard_key, &plan, None).await?, &req.callable);
    };

    // An out-of-band store (in-process `MemStore`, a Redis store): fingerprint the request
    // payload (args + `$ctx`) so the store can tell a genuine retry (same payload) from one
    // key reused for a different request, then bracket the mutation with begin/record.
    match store.begin(&req.callable, key, req.fingerprint()).await {
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
            let response =
                plain_outcome(apply(backend, shard_key, &plan, None).await?, &req.callable)?;
            claim.armed = false;
            store.record(&req.callable, key, response.clone()).await;
            Ok(response)
        }
    }
}

/// Map a mutation-attempt [`TxOutcome`] for a path that carried **no** in-transaction claim
/// (keyless, or an out-of-band store): only `Done`/`NotFound` can arise — `Replayed` and
/// `Mismatch` are produced solely by a tx-participant claim.
fn plain_outcome(outcome: TxOutcome, callable: &str) -> Result<serde_json::Value, RunError> {
    match outcome {
        TxOutcome::Done(r) => Ok(r),
        TxOutcome::NotFound => Err(RunError::NotFound(callable.to_string())),
        TxOutcome::Replayed(_) | TxOutcome::Mismatch => {
            unreachable!("replay/mismatch require an in-transaction claim context")
        }
    }
}

/// The context a tx-participant store ([`TxIdempotency`]) needs to claim + record its key
/// inside the mutation's own transaction (atomic exactly-once).
struct TxClaimCtx<'a> {
    participant: &'a dyn TxIdempotency,
    callable: &'a str,
    key: &'a str,
    fingerprint: Fingerprint,
}

/// The result of one mutation-transaction attempt ([`apply_once`]).
enum TxOutcome {
    /// The write body ran and produced this response (recorded in-tx when a tx-participant
    /// store claimed the key).
    Done(serde_json::Value),
    /// A prior committed attempt under the same idempotency key already recorded this
    /// response; it was replayed with no writes (tx-participant store only).
    Replayed(serde_json::Value),
    /// The idempotency key was reused for a different request — fingerprint mismatch
    /// (tx-participant store only).
    Mismatch,
    /// The write's `where` matched no row — nothing was written (the transaction rolled
    /// back).
    NotFound,
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
/// The [`TxOutcome`] (including a matched-no-row `NotFound`) is passed through. When
/// `claim` is present the whole attempt — key claim, writes, and response record — retries
/// as one unit, so a re-run after a deadlock re-reads the key and replays if a sibling
/// attempt has since committed it.
async fn apply(
    backend: &dyn Backend,
    shard_key: &str,
    plan: &MutationPlan,
    claim: Option<&TxClaimCtx<'_>>,
) -> Result<TxOutcome, DbError> {
    let mut attempt = 0u32;
    loop {
        let db = backend.checkout(shard_key).await?;
        match apply_once(db, plan, claim).await {
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
    claim: Option<&TxClaimCtx<'_>>,
) -> Result<TxOutcome, DbError> {
    let mut tx = db.begin().await?;

    // Strong-form idempotency: claim the key on the mutation's own connection, inside this
    // transaction, before any write. A concurrent retry blocks here on the first,
    // still-uncommitted attempt (the key's unique index) and replays its result once that
    // transaction commits — so a done/mismatch here drops `tx` (rollback, nothing written).
    if let Some(c) = claim {
        match c
            .participant
            .claim(&mut *tx, c.callable, c.key, c.fingerprint)
            .await?
        {
            TxClaim::Fresh => {}
            TxClaim::Done(resp) => return Ok(TxOutcome::Replayed(resp)),
            TxClaim::Mismatch => return Ok(TxOutcome::Mismatch),
        }
    }

    let response = match run_writes(&mut *tx, plan).await? {
        TxOutcome::Done(r) => r,
        // A matched-no-row miss drops `tx` (rollback), nothing written.
        other => return Ok(other),
    };
    // Record the response inside the same transaction, before commit, so the key, the
    // writes, and the response commit atomically (or roll back together).
    if let Some(c) = claim {
        c.participant
            .record(&mut *tx, c.callable, c.key, &response)
            .await?;
    }
    tx.commit().await?;
    Ok(TxOutcome::Done(response))
}

/// Run a mutation plan's writes in order on an already-open connection/transaction, then
/// assemble the declared-shape read-back — the write core shared by the auto-committing
/// [`apply_once`] (which brackets it with begin/commit + the idempotency claim) and the
/// host-transaction [`run_mutation_on`] (which runs it on a caller-owned [`Tx`], committing
/// nothing). Takes any [`DbRead`], so a `&mut dyn Tx` passes straight in. Returns
/// [`TxOutcome::Done`] with the response, or [`TxOutcome::NotFound`] when the write's
/// `where` (with its scope/soft-delete guards) matched no row — the caller decides the
/// rollback.
async fn run_writes<D: DbRead + ?Sized>(
    db: &mut D,
    plan: &MutationPlan,
) -> Result<TxOutcome, DbError> {
    use crate::scan::to_positional;
    use crate::value::coerce;
    use serde_json::Value as J;

    // The value environment accumulates as the writes run: a bound create's row read-back
    // captures committed column values a later step (or the declared re-select) binds — so
    // every step is bound late, from this growing environment (D124).
    let mut env = plan.env0.clone();
    let bind = |sql: &str, env: &std::collections::HashMap<String, SqlValue>| {
        to_positional(sql, plan.dialect, |name| {
            env.get(name).cloned().map(SqlValue::expand)
        })
        .map_err(|n| DbError::new(format!("unbound placeholder `:{n}` (planner mismatch)")))
    };

    // A structured `create … from` with a declared shape return (BW1b/BW2) reads its written
    // rows back keyed on their keys — app-known from the payload, or DB-generated (`serial`)
    // ids learned from the INSERT. Captured here, replayed after the writes run.
    let mut readback_keys: Vec<Vec<SqlValue>> = Vec::new();

    for (i, step) in plan.steps.iter().enumerate() {
        // A structured shape-input create (BW1): materialize a chunked, atomic multi-row
        // INSERT from the plan's resolved rows. Returns the DB-generated ids for a `serial`
        // read-back (empty otherwise).
        if let Some(bulk) = &step.bulk {
            // Only recover this insert's keys when a read-back will consume them (a plain
            // `-> ok` insert needs no `RETURNING` / `LAST_INSERT_ID()` round-trip). Nested
            // children always recover their key — the parent links to it.
            let want_pk = plan.bulk_readback.is_some();
            let pk_rows = exec_bulk(db, plan.dialect, bulk, &env, want_pk, &[]).await?;
            if plan.bulk_readback.is_some() {
                readback_keys = if bulk.serial_return.is_some() {
                    pk_rows
                } else {
                    bulk.key_rows.clone()
                };
            }
            continue;
        }
        let (sql, params) = bind(&step.sql, &env)?;
        // A bound create's row read-back captures the written row's committed columns
        // (the INSERT's own `RETURNING`, or a MySQL follow-up keyed `SELECT`) into `env`; a
        // plain write just executes (and, for an `-> ok` DELETE, checks it touched a row).
        let Some(cap) = &step.capture else {
            let affected = db.execute(&sql, &params).await?;
            if plan.ack_check == Some(i) && affected == 0 {
                return Ok(TxOutcome::NotFound);
            }
            continue;
        };
        let row = if let Some(sel) = &cap.followup_select {
            db.execute(&sql, &params).await?;
            let (ssql, sparams) = bind(sel, &env)?;
            fetch_all(db.fetch(&ssql, &sparams))
                .await?
                .into_iter()
                .next()
        } else {
            fetch_all(db.fetch(&sql, &params)).await?.into_iter().next()
        };
        let row = row.ok_or_else(|| DbError::new("bound create read-back returned no row"))?;
        for c in &cap.cols {
            let v = row.get(&c.column).cloned().unwrap_or(J::Null);
            let bound = coerce(&v, c.family, true)
                .map_err(|e| DbError::new(format!("capture `{}`: {e:?}", c.column)))?;
            env.insert(c.bind.clone(), bound);
        }
    }

    // A structured `create … from` reads its written rows back in the declared shape via an
    // IN-keyed re-select over the captured keys, returned in input order (BW1b/BW2).
    if let Some(rb) = &plan.bulk_readback {
        return run_bulk_readback(db, plan, rb, &readback_keys, &env).await;
    }

    // Read the written row back in its declared shape, bound late from the accumulated
    // environment (its `:result_id` is app-minted in `env0` or captured above).
    let response = match &plan.ret_select {
        Some(sql) => {
            let (sql, params) = bind(sql, &env)?;
            let rows = fetch_all(db.fetch(&sql, &params)).await?;
            match rows.into_iter().next() {
                Some(row) => {
                    let mut v = nest_row(row);
                    normalize_json(&mut v, &plan.json_paths);
                    v
                }
                None => return Ok(TxOutcome::NotFound),
            }
        }
        // No declared-shape re-select (the row did not survive — a real DELETE):
        // identify the created row by its engine `id`, or `{}` when nothing was created.
        None => match env.get("result_id") {
            Some(v) => {
                let mut obj = serde_json::Map::new();
                obj.insert("id".into(), sql_value_to_json(v));
                J::Object(obj)
            }
            None => J::Object(serde_json::Map::new()),
        },
    };
    Ok(TxOutcome::Done(response))
}

/// The per-dialect ceiling on bound parameters in one statement, above which a bulk INSERT
/// is chunked (BW1). Postgres's wire protocol caps at 65535; SQLite's compile-time variable
/// limit is smaller and version-dependent, so a conservative value keeps every build safe.
/// The user never sees the cap — the engine chunks transparently.
fn max_binds(dialect: based_codegen::Dialect) -> usize {
    match dialect {
        based_codegen::Dialect::Sqlite => 900,
        _ => 65000,
    }
}

/// A per-row override for one FK column of a nested-write step: the column's bind index and
/// its value in each row (the parent's key, for a to-many child).
struct ColInject {
    bind: usize,
    per_row: Vec<SqlValue>,
}

/// Execute a structured shape-input create and its nested-write children (BW1 + nested
/// writes). Order: create **to-one** children first (their key fills this insert's FK), run
/// this insert, then create **to-many** children (this insert's key fills their back-FK).
/// `inject` supplies FK values this step's parent computed (a to-many child's back-FK).
/// Returns each written row's primary key (in `pk_recover` order) when `want_pk`, so a
/// parent can link to it. Every insert runs on the same transaction connection, so the whole
/// nested write is atomic with the surrounding mutation.
async fn exec_bulk<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: based_codegen::Dialect,
    step: &crate::plan::BulkStep,
    env: &std::collections::HashMap<String, SqlValue>,
    want_pk: bool,
    inject: &[ColInject],
) -> Result<Vec<Vec<SqlValue>>, DbError> {
    // A fillable copy of the rows — nested-write FK columns are overwritten before insert.
    let mut rows = step.rows.clone();
    // Parent-supplied back-FK values (a to-many child links to its parent's key).
    for ci in inject {
        for (i, v) in ci.per_row.iter().enumerate() {
            if let Some(row) = rows.get_mut(i) {
                if ci.bind < row.len() {
                    row[ci.bind] = v.clone();
                }
            }
        }
    }
    // To-one children first: create each, then splice its key into this insert's FK.
    for nc in &step.nested_one {
        let child_pks = Box::pin(exec_bulk(&mut *db, dialect, &nc.step, env, true, &[])).await?;
        for (j, &parent) in nc.parent_of.iter().enumerate() {
            for slot in &nc.link_slots {
                let Some(idx) = nc
                    .step
                    .pk_recover
                    .iter()
                    .position(|p| p.field == slot.key_field)
                else {
                    continue;
                };
                if let (Some(prow), Some(cval)) = (
                    rows.get_mut(parent),
                    child_pks.get(j).and_then(|pk| pk.get(idx)),
                ) {
                    if slot.bind < prow.len() {
                        prow[slot.bind] = cval.clone();
                    }
                }
            }
        }
    }

    // This insert must expose its key when a caller wants it OR a to-many child links to it.
    let need_pk = want_pk || !step.nested_many.is_empty();
    let want_serial = need_pk && step.serial_return.is_some();
    let serial_ids = insert_bulk_rows(db, dialect, step, &rows, env, want_serial).await?;

    let pk_rows: Vec<Vec<SqlValue>> = if need_pk {
        rows.iter()
            .enumerate()
            .map(|(i, row)| {
                step.pk_recover
                    .iter()
                    .map(|p| match p.bind {
                        Some(b) => row[b].clone(),
                        None => serial_ids.get(i).cloned().unwrap_or(SqlValue::Null),
                    })
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };

    // To-many children after: this insert's key fills each child's back-FK, per child row.
    for nm in &step.nested_many {
        let child_inject: Vec<ColInject> = nm
            .link_slots
            .iter()
            .filter_map(|slot| {
                let idx = step
                    .pk_recover
                    .iter()
                    .position(|p| p.field == slot.key_field)?;
                let per_row = nm
                    .parent_of
                    .iter()
                    .map(|&parent| {
                        pk_rows
                            .get(parent)
                            .and_then(|pk| pk.get(idx))
                            .cloned()
                            .unwrap_or(SqlValue::Null)
                    })
                    .collect();
                Some(ColInject {
                    bind: slot.bind,
                    per_row,
                })
            })
            .collect();
        Box::pin(exec_bulk(
            &mut *db,
            dialect,
            &nm.step,
            env,
            false,
            &child_inject,
        ))
        .await?;
    }

    if want_pk {
        Ok(pk_rows)
    } else {
        Ok(Vec::new())
    }
}

/// Insert already-resolved rows as one or more multi-row `INSERT … VALUES (…),(…),…`
/// statements, transparently chunked below the driver's bind limit. Recovers the
/// DB-generated `serial` id per row when `want_serial` (`RETURNING` on Postgres/SQLite, the
/// `LAST_INSERT_ID()` range on MySQL/MariaDB). Zero rows is a no-op.
async fn insert_bulk_rows<D: DbRead + ?Sized>(
    db: &mut D,
    dialect: based_codegen::Dialect,
    step: &crate::plan::BulkStep,
    rows: &[Vec<SqlValue>],
    env: &std::collections::HashMap<String, SqlValue>,
    want_serial: bool,
) -> Result<Vec<SqlValue>, DbError> {
    use based_codegen::Dialect;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let col_list = step
        .columns
        .iter()
        .map(|c| c.quoted.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let per_row = step.binds_per_row.max(1);
    // A bulk upsert's tail binds (a param / `$ctx`) repeat once per chunk statement, so they
    // count against the chunk's bind budget alongside the per-row binds.
    let tail_binds = step.conflict_tail.as_ref().map_or(0, |t| count_named(t));
    let chunk_rows = ((max_binds(dialect).saturating_sub(tail_binds)) / per_row).max(1);
    // The `serial` id uses `RETURNING` on Postgres/SQLite; MySQL/MariaDB have none, so the
    // ids come from the `LAST_INSERT_ID()` range (the first id + a contiguous block).
    let returning = if want_serial {
        step.serial_return.as_ref()
    } else {
        None
    };
    let use_returning =
        returning.is_some() && matches!(dialect, Dialect::Postgres | Dialect::Sqlite);
    let mut serial_ids: Vec<SqlValue> = Vec::new();

    for chunk in rows.chunks(chunk_rows) {
        let mut sql = format!("INSERT INTO {} ({col_list})\nVALUES ", step.table);
        let mut params: Vec<SqlValue> = Vec::with_capacity(chunk.len() * per_row + tail_binds);
        let mut ord = 0usize;
        for (r, row) in chunk.iter().enumerate() {
            if r > 0 {
                sql.push_str(", ");
            }
            sql.push('(');
            let mut bind_i = 0usize;
            for (ci, c) in step.columns.iter().enumerate() {
                if ci > 0 {
                    sql.push_str(", ");
                }
                if let Some(lit) = &c.literal {
                    sql.push_str(lit);
                } else {
                    ord += 1;
                    match dialect {
                        Dialect::Postgres => {
                            sql.push('$');
                            sql.push_str(&ord.to_string());
                        }
                        _ => sql.push('?'),
                    }
                    params.push(row[bind_i].clone());
                    bind_i += 1;
                }
            }
            sql.push(')');
        }
        // Bulk upsert (BW2): the `ON CONFLICT … / ON DUPLICATE KEY UPDATE` tail, its `:name`
        // binds continuing this statement's positional count.
        if let Some(tail) = &step.conflict_tail {
            let (frag, tparams) = crate::scan::to_positional_from(tail, dialect, ord, |n| {
                env.get(n).cloned().map(SqlValue::expand)
            })
            .map_err(|n| DbError::new(format!("unbound placeholder `:{n}` (upsert tail)")))?;
            sql.push_str(&frag);
            params.extend(tparams);
        }
        if use_returning {
            let scol = returning.unwrap();
            sql.push_str(&format!(" RETURNING {}", dialect.quote(scol)));
            sql.push_str(";\n");
            let rows = fetch_all(db.fetch(&sql, &params)).await?;
            for mut row in rows {
                let v = row.remove(scol).unwrap_or(serde_json::Value::Null);
                serial_ids.push(json_to_key(&v));
            }
        } else if returning.is_some() {
            // MySQL/MariaDB: no `INSERT … RETURNING` — the ids are the contiguous
            // `LAST_INSERT_ID()` range (first id .. first + chunk length).
            sql.push_str(";\n");
            db.execute(&sql, &params).await?;
            let first = fetch_all(db.fetch("SELECT LAST_INSERT_ID() AS id", &[]))
                .await?
                .into_iter()
                .next()
                .and_then(|r| r.get("id").and_then(serde_json::Value::as_i64))
                .ok_or_else(|| DbError::new("LAST_INSERT_ID() returned no row"))?;
            for i in 0..chunk.len() as i64 {
                serial_ids.push(SqlValue::Int(first + i));
            }
        } else {
            sql.push_str(";\n");
            db.execute(&sql, &params).await?;
        }
    }
    Ok(serial_ids)
}

/// Count the `:name` placeholders in a SQL fragment (quote-aware via [`to_positional`]) —
/// how many binds a bulk upsert's tail contributes to each chunk statement.
fn count_named(sql: &str) -> usize {
    crate::scan::to_positional(sql, based_codegen::Dialect::Postgres, |_| {
        Some(crate::scan::Bound::One(()))
    })
    .map_or(0, |(_, v)| v.len())
}

/// A read-back key value from a fetched JSON scalar: an integer id stays an `Int`, anything
/// else its text — enough for equality-keying rows back to their input order.
fn json_to_key(v: &serde_json::Value) -> SqlValue {
    match v {
        serde_json::Value::Number(n) if n.is_i64() => SqlValue::Int(n.as_i64().unwrap()),
        serde_json::Value::Number(n) if n.is_u64() => SqlValue::Int(n.as_u64().unwrap() as i64),
        serde_json::Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// The declared-shape read-back for a structured `create … from` (BW1b/BW2): re-select the
/// written rows keyed on their keys (an `IN (…)` over the captured key tuples), reorder them
/// to input order via the hidden `__bkk_<i>` key columns, and return one object (`-> Shape`)
/// or an array (`-> Shape[]`). Reuses the shape projection, so nested shapes decode as on a
/// read. Zero written rows → an empty array (bulk) / not-found (single).
async fn run_bulk_readback<D: DbRead + ?Sized>(
    db: &mut D,
    plan: &MutationPlan,
    rb: &crate::plan::BulkReadbackPlan,
    keys: &[Vec<SqlValue>],
    env: &std::collections::HashMap<String, SqlValue>,
) -> Result<TxOutcome, DbError> {
    use serde_json::Value as J;
    if keys.is_empty() {
        return if rb.bulk {
            Ok(TxOutcome::Done(J::Array(Vec::new())))
        } else {
            Ok(TxOutcome::NotFound)
        };
    }

    // Splice the key-tuple IN-list into the sentinel, as `:__bk_<row>_<col>` binds.
    let mut binds: std::collections::HashMap<String, SqlValue> = std::collections::HashMap::new();
    let mut tuples: Vec<String> = Vec::with_capacity(keys.len());
    for (r, key) in keys.iter().enumerate() {
        let parts: Vec<String> = key
            .iter()
            .enumerate()
            .map(|(c, v)| {
                let name = format!("__bk_{r}_{c}");
                binds.insert(name.clone(), v.clone());
                format!(":{name}")
            })
            .collect();
        tuples.push(if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            format!("({})", parts.join(", "))
        });
    }
    let sql = rb.sql.replace(
        based_codegen::sql::mutations::BULK_KEYS_SENTINEL,
        &tuples.join(", "),
    );
    let (bound_sql, params) = crate::scan::to_positional(&sql, plan.dialect, |n| {
        env.get(n)
            .or_else(|| binds.get(n))
            .cloned()
            .map(SqlValue::expand)
    })
    .map_err(|n| DbError::new(format!("unbound placeholder `:{n}` (bulk read-back)")))?;
    let rows = fetch_all(db.fetch(&bound_sql, &params)).await?;

    // Map each fetched row by its hidden key columns (stripped before nesting), then emit in
    // input-key order (a duplicate input key repeats its row; a missing key is skipped).
    let mut by_key: std::collections::HashMap<String, J> = std::collections::HashMap::new();
    let alias_prefix = based_codegen::sql::mutations::BULK_KEY_ALIAS;
    for mut row in rows {
        let mut kparts: Vec<J> = Vec::with_capacity(rb.key_count);
        for c in 0..rb.key_count {
            kparts.push(row.remove(&format!("{alias_prefix}{c}")).unwrap_or(J::Null));
        }
        let mut v = nest_row(row);
        normalize_json(&mut v, &plan.json_paths);
        by_key.insert(norm_key(&kparts), v);
    }
    let mut out: Vec<J> = Vec::with_capacity(keys.len());
    for key in keys {
        let kparts: Vec<J> = key.iter().map(sql_value_to_json).collect();
        if let Some(v) = by_key.get(&norm_key(&kparts)) {
            out.push(v.clone());
        }
    }

    if rb.bulk {
        Ok(TxOutcome::Done(J::Array(out)))
    } else {
        match out.into_iter().next() {
            Some(v) => Ok(TxOutcome::Done(v)),
            None => Ok(TxOutcome::NotFound),
        }
    }
}

/// A canonical string key for a tuple of JSON scalars — equal iff the values are equal — so a
/// fetched row's hidden key columns match the captured input key regardless of source.
fn norm_key(parts: &[serde_json::Value]) -> String {
    parts
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// The `{ id }` fallback response value for a created row's id — a uuid/ulid string or a
/// DB-generated integer, mirroring its wire type.
fn sql_value_to_json(v: &SqlValue) -> serde_json::Value {
    match v {
        SqlValue::Int(i) => serde_json::Value::Number((*i).into()),
        SqlValue::Uuid(s) | SqlValue::Text(s) => serde_json::Value::String(s.clone()),
        other => serde_json::json!(format!("{other:?}")),
    }
}

/// Plan and run a mutation on a caller-owned open transaction — the host-language
/// read-decide-write seam (transactions.md). The writes run on `db` (a `&mut dyn Tx`) with
/// **no** begin/commit and **no** idempotency claim: the caller owns the transaction
/// boundary (the managed closure commits on `Ok` / rolls back on `Err`, or the explicit
/// handle's `commit`/`rollback` does). A matched-no-row write is a [`RunError::NotFound`]
/// the caller surfaces (and rolls the whole transaction back on).
pub async fn run_mutation_on<D: DbRead + ?Sized>(
    compiled: &Compiled,
    db: &mut D,
    id_gen: &dyn IdGen,
    req: &Request,
) -> Result<serde_json::Value, RunError> {
    let plan = plan_mutation(compiled, req, id_gen)?;
    match run_writes(db, &plan).await? {
        TxOutcome::Done(r) => Ok(r),
        TxOutcome::NotFound => Err(RunError::NotFound(req.callable.clone())),
        TxOutcome::Replayed(_) | TxOutcome::Mismatch => {
            unreachable!("the in-transaction write path carries no idempotency claim")
        }
    }
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

/// Normalize every `json`-typed leaf named by `paths` in one already-nested result row.
/// A `json` column stores JSON as text (SQLite/MariaDB), so a driver reads it back as a
/// JSON *string*; parsing it here yields the structured object/array it holds, so a `json`
/// field round-trips as what was written, not a double-encoded string. A value already
/// structured (a driver that decodes json natively) is left untouched, and a string that
/// does not parse as JSON is left as-is (never a panic).
fn normalize_json(row: &mut serde_json::Value, paths: &[String]) {
    for path in paths {
        let segs: Vec<&str> = path.split(based_codegen::sql::NEST_SEP).collect();
        parse_json_at(row, &segs);
    }
}

/// Descend `value` along `segs` (a `.`-split json path; a `field[]` segment is an array to
/// recurse into per element) and, at the leaf, parse a JSON string into structured JSON.
fn parse_json_at(value: &mut serde_json::Value, segs: &[&str]) {
    use serde_json::Value as J;
    let Some((head, rest)) = segs.split_first() else {
        if let J::String(s) = value {
            if let Ok(parsed) = serde_json::from_str::<J>(s) {
                *value = parsed;
            }
        }
        return;
    };
    match head.strip_suffix(ARRAY_MARK) {
        Some(key) => {
            if let J::Object(map) = value {
                if let Some(J::Array(arr)) = map.get_mut(key) {
                    for elem in arr.iter_mut() {
                        parse_json_at(elem, rest);
                    }
                }
            }
        }
        None => {
            if let J::Object(map) = value {
                if let Some(child) = map.get_mut(*head) {
                    parse_json_at(child, rest);
                }
            }
        }
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
    // Nest a flat row into the response object, then normalize its `json` leaves (a json
    // column read back as a text string → structured JSON).
    let paths = &plan.json_paths;
    let nest = |row: Row| {
        let mut v = nest_row(row);
        if !paths.is_empty() {
            normalize_json(&mut v, paths);
        }
        v
    };
    Ok(match plan.envelope {
        // `get`: the first row, or JSON null (Option<T>).
        Envelope::One => rows.into_iter().next().map_or(J::Null, nest),
        // `list`: every row as an array.
        Envelope::Many => J::Array(rows.into_iter().map(nest).collect()),
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
            obj.insert("rows".into(), J::Array(rows.into_iter().map(nest).collect()));
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
    /// The next this-many `execute` calls fail with a [`DbErrorKind::Deadlock`] (then
    /// succeed) — the injected serialization abort the transaction-retry path re-runs on.
    deadlock_writes: usize,
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

    /// Make the next `n` `execute` calls fail with a [`DbErrorKind::Deadlock`] before
    /// succeeding — an injected serialization/deadlock abort the transaction-retry path
    /// (`transaction_retrying`) re-runs the whole transaction on.
    pub fn deadlocking(self, n: usize) -> Self {
        self.state.lock().unwrap().deadlock_writes = n;
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
        {
            let mut st = self.state.lock().unwrap();
            if st.deadlock_writes > 0 {
                st.deadlock_writes -= 1;
                return Err(DbError::of(
                    DbErrorKind::Deadlock,
                    "mock serialization failure",
                ));
            }
        }
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
