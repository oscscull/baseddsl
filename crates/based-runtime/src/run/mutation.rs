use super::*;

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
pub(crate) fn plain_outcome(outcome: TxOutcome, callable: &str) -> Result<serde_json::Value, RunError> {
    match outcome {
        TxOutcome::Done(r) => Ok(r),
        TxOutcome::NotFound => Err(RunError::NotFound(callable.to_string())),
        TxOutcome::Replayed(_) | TxOutcome::Mismatch => {
            unreachable!("replay/mismatch require an in-transaction claim context")
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

/// Backoff before re-running a deadlocked transaction: a short exponential step (capped
/// at 100ms — a deadlock clears in milliseconds once the winner commits) plus jitter, so
/// two transactions that just deadlocked don't retry in lockstep and collide again.
pub(crate) fn deadlock_backoff(attempt: u32) -> std::time::Duration {
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
pub(crate) async fn apply(
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
pub(crate) async fn apply_once(
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
pub(crate) async fn run_writes<D: DbRead + ?Sized>(
    db: &mut D,
    plan: &MutationPlan,
) -> Result<TxOutcome, DbError> {
    use crate::scan::to_positional;
    use crate::value::coerce;
    use serde_json::Value as J;

    // The value environment accumulates as the writes run: a bound create's row read-back
    // captures committed column values a later step (or the declared re-select) binds — so
    // every step is bound late, from this growing environment.
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

/// Plan and run a mutation on a caller-owned open transaction — the host-language
/// read-decide-write seam. The writes run on `db` (a `&mut dyn Tx`) with
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

/// The context a tx-participant store ([`TxIdempotency`]) needs to claim + record its key
/// inside the mutation's own transaction (atomic exactly-once).
pub(crate) struct TxClaimCtx<'a> {
    participant: &'a dyn TxIdempotency,
    callable: &'a str,
    key: &'a str,
    fingerprint: Fingerprint,
}

/// The result of one mutation-transaction attempt ([`apply_once`]).
pub(crate) enum TxOutcome {
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

/// How many times the mutation path re-runs a transaction the server aborted for a
/// deadlock / serialization conflict before giving up. Bounded so a pathological hot row
/// fails fast as a `503` rather than retrying forever; a handful of attempts clears an
/// ordinary two-transaction deadlock (the loser re-runs after the winner commits). Total
/// attempts = 1 + this.
const TX_RETRY_LIMIT: u32 = 5;
