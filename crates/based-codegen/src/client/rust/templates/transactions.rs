/// The `TxBound` marker trait + the header of the transaction-confined client impl block.
/// A locking read (`for update`) is emitted in `impl<T: Transport + TxBound> Client<T>`, so
/// it is callable only on a transport that carries `TxBound` — only `TxTransport` does (the
/// [`TX_BOUND_IMPL`] under the embedded bridge). Declared unconditionally with the block so a
/// pure-wire build (no `based_runtime`, no `TxTransport`) still compiles — the locking methods
/// are simply uncallable there, no transport implementing `TxBound` exists.
pub(crate) const TXBOUND_TRAIT: &str = r#"
// ---------- locking reads: transaction-confined (`for update`) ----------

/// Marker for a transport a `for update` locking read may run on — a transaction-bound
/// transport, where a `SELECT … FOR UPDATE` holds the lock to the transaction boundary.
/// Implemented only by `TxTransport`; the auto-commit `Embedded`/wire transports do not
/// carry it, so a locking method is a compile error on their client.
pub trait TxBound {}

"#;

/// `TxTransport` is the transaction-bound transport, so it carries `TxBound` — the one
/// transport a locking read may run through. Emitted under the embedded gate (where
/// `TxTransport` exists) when the schema has a `for update` query.
pub(crate) const TX_BOUND_IMPL: &str = r#"
impl TxBound for based_runtime::TxTransport {}
"#;

/// The transaction seam's head, appended after the embedded bridge (embedded gate): the
/// `Transport` impl over `based_runtime::TxTransport` — the same generated client, run
/// on a held transaction. The keyed/stream doors ([`TX_TRANSPORT_KEYED_CALL`],
/// [`TX_TRANSPORT_STREAM_CALL`]) splice in before [`TX_SURFACE_TAIL`] exactly as the
/// wire transport's optional doors do.
pub(crate) const TX_TRANSPORT_HEAD: &str = r#"
// ---------- transactions: read-decide-write seam (in-process) ----------

/// A `Transport` bound to an open `based_runtime::Transaction`: every callable runs on the
/// held transaction (committing nothing — the transaction owns the boundary). Because the
/// client is generic over `Transport`, every generated method works inside a transaction
/// with no extra codegen. The `call` body mirrors the `Embedded` bridge; only the
/// connection differs (the held transaction, via `TxTransport::dispatch`).
impl Transport for based_runtime::TxTransport {
    async fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        let args = serde_json::to_value(input).map_err(ClientError::decode)?;
        // `&()` → JSON `null`; the engine treats a non-object context as empty.
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { serde_json::json!({}) })
            .map_err(ClientError::decode)?;
        let resp = self.dispatch(route, args, ctx).await;
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(ClientError::decode)
        } else {
            let code = resp.body["error"]["code"].as_str().unwrap_or("error");
            let message = resp.body["error"]["message"].as_str().unwrap_or("call failed");
            Err(ClientError::api(resp.status, code, message))
        }
    }
"#;

/// The transaction transport's keyed door (schema with a mutation): a keyed mutation
/// inside a transaction runs on the transaction — the enclosing transaction is the
/// atomicity boundary, so the idempotency key is not separately consulted.
pub(crate) const TX_TRANSPORT_KEYED_CALL: &str = r#"
    /// A keyed mutation inside a transaction: the enclosing transaction is the exactly-once
    /// boundary, so the write runs on it directly and the key is not separately consulted.
    async fn call_with_key<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
        _key: &str,
    ) -> Result<O, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        self.call(route, input, ctx).await
    }
"#;

/// The transaction transport's streaming door (schema with a `-> stream` query):
/// rejected in this slice — a stream borrows the connection for its whole life, which
/// the transaction holds — rather than silently run outside the transaction.
pub(crate) const TX_TRANSPORT_STREAM_CALL: &str = r#"
    /// Streaming within an explicit transaction is not supported in this slice (a stream
    /// holds the connection for its whole life, which the transaction owns): rejected
    /// loudly rather than run outside the transaction.
    async fn call_stream<I, C, O>(
        &self,
        _route: &str,
        _input: &I,
        _ctx: &C,
    ) -> Result<RowStream<O>, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned + Send + 'static,
    {
        Err(ClientError::api(
            500,
            "unsupported",
            "streaming within a transaction is not supported",
        ))
    }
"#;

/// Closes the transaction `Transport` impl and adds the user-facing surface: `TxError`,
/// `Retry`, the managed-closure `transaction` / `transaction_retrying` entry points, and
/// the `TransactionExt::client()` accessor for the explicit `Transaction` handle.
pub(crate) const TX_SURFACE_TAIL: &str = r#"}

/// The error a transaction closure returns. A database serialization/deadlock abort is
/// `retryable` (`transaction_retrying` re-runs the whole closure on it); a failed client
/// call inside the transaction, or an application error, is not. Client calls (`?` inside
/// the closure) and `Engine::begin` convert into this automatically.
#[derive(Debug)]
pub enum TxError {
    /// A database operational failure opening or committing the transaction.
    Db(based_runtime::DbError),
    /// A client call inside the transaction failed (a server error, decode, or transport).
    Client(ClientError),
    /// An application error the closure chose to abort (and roll back) with.
    App(Box<dyn std::error::Error + Send + Sync>),
}

impl TxError {
    /// Wrap an application error to abort the transaction with (rolls back).
    pub fn app(err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        TxError::App(err.into())
    }
    /// Is this a serialization/deadlock failure the engine may safely retry?
    pub fn is_retryable(&self) -> bool {
        match self {
            TxError::Db(e) => e.is_deadlock(),
            TxError::Client(e) => e.code() == "deadlock",
            TxError::App(_) => false,
        }
    }
}

impl From<based_runtime::DbError> for TxError {
    fn from(e: based_runtime::DbError) -> Self {
        TxError::Db(e)
    }
}
impl From<ClientError> for TxError {
    fn from(e: ClientError) -> Self {
        TxError::Client(e)
    }
}
impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxError::Db(e) => write!(f, "transaction database error: {e}"),
            TxError::Client(e) => write!(f, "transaction call error: {e}"),
            TxError::App(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for TxError {}

/// How many times `transaction_retrying` re-runs the closure on a serialization/deadlock
/// abort before giving up.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub max: u32,
}
impl Retry {
    /// Retry up to `max` times on a serialization/deadlock failure.
    pub fn on_serialization(max: u32) -> Self {
        Retry { max }
    }
}

/// Get a `Client` bound to an open transaction handle — `txn.client()`. Import this trait
/// to call it on a `based_runtime::Transaction` (the explicit-handle rung).
pub trait TransactionExt {
    fn client(&self) -> Client<based_runtime::TxTransport>;
}
impl TransactionExt for based_runtime::Transaction {
    fn client(&self) -> Client<based_runtime::TxTransport> {
        Client {
            transport: self.transport(),
        }
    }
}

/// Run `f` inside an **engine-owned** transaction (the safe default, rung 1): open a
/// transaction at `opts`, hand the closure a `Client` bound to it, then **commit on `Ok`,
/// roll back on `Err`/panic, always release**. The closure's client calls run on the
/// transaction — read a row, decide in host Rust, write — all atomic.
pub async fn transaction<R, Fut, F>(
    engine: &based_runtime::Engine,
    opts: based_runtime::TxOptions,
    f: F,
) -> Result<R, TxError>
where
    F: FnOnce(Client<based_runtime::TxTransport>) -> Fut,
    Fut: std::future::Future<Output = Result<R, TxError>>,
{
    let txn = engine.begin(opts).await?;
    let client = Client {
        transport: txn.transport(),
    };
    match f(client).await {
        Ok(value) => {
            txn.commit().await?;
            Ok(value)
        }
        Err(err) => {
            let _ = txn.rollback().await;
            Err(err)
        }
    }
}

/// Like `transaction`, but re-runs the whole closure when the database aborts it with a
/// serialization/deadlock failure (up to `retry.max` times, with backoff) — the
/// optimistic-concurrency payoff of `Serializable`. Only the engine-owned form can
/// auto-retry, since it owns the boundary.
pub async fn transaction_retrying<R, Fut, F>(
    engine: &based_runtime::Engine,
    opts: based_runtime::TxOptions,
    retry: Retry,
    mut f: F,
) -> Result<R, TxError>
where
    F: FnMut(Client<based_runtime::TxTransport>) -> Fut,
    Fut: std::future::Future<Output = Result<R, TxError>>,
{
    let mut attempt: u32 = 0;
    loop {
        let txn = engine.begin(opts).await?;
        let client = Client {
            transport: txn.transport(),
        };
        match f(client).await {
            Ok(value) => match txn.commit().await {
                Ok(()) => return Ok(value),
                Err(e) if e.is_deadlock() && attempt < retry.max => {
                    attempt += 1;
                    tx_retry_backoff(attempt).await;
                }
                Err(e) => return Err(TxError::Db(e)),
            },
            Err(err) if err.is_retryable() && attempt < retry.max => {
                let _ = txn.rollback().await;
                attempt += 1;
                tx_retry_backoff(attempt).await;
            }
            Err(err) => {
                let _ = txn.rollback().await;
                return Err(err);
            }
        }
    }
}

/// Short capped exponential backoff between transaction retries, so two conflicting
/// transactions don't re-collide in lockstep.
async fn tx_retry_backoff(attempt: u32) {
    let step = std::cmp::min(2u64.saturating_pow(attempt).saturating_mul(2), 100);
    tokio::time::sleep(std::time::Duration::from_millis(step)).await;
}
"#;
