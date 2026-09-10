/// The abstract transport's head: doc, trait open, and the one `call` every schema
/// gets. The optional doors ([`TRANSPORT_CALL_WITH_KEY`], [`TRANSPORT_CALL_STREAM`])
/// splice in before [`TRANSPORT_TAIL`], so a schema without them emits the exact
/// module head earlier versions did.
pub(crate) const TRANSPORT_HEAD: &str = r#"
/// Post a typed input to a route, carry the typed request context (`$ctx`, carried out
/// of band as request context), and decode the typed output. A callable with no `$ctx`
/// requirements passes `ctx: &()`. Async: a transport awaits its round-trip (an HTTP
/// client's socket, or the in-process engine's execution). Codegen only depends on
/// this shape.
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn call<I, C, O>(&self, route: &str, input: &I, ctx: &C) -> Result<O, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned;
"#;

/// The keyed mutation door, emitted only for a schema with a mutation: the same
/// call carrying an idempotency key out of band. Required (no default body) — a
/// transport must decide how to carry the key, never silently drop it.
pub(crate) const TRANSPORT_CALL_WITH_KEY: &str = r#"
    /// Like [`call`](Transport::call), carrying a mutation **idempotency key** out of
    /// band — an HTTP transport sends it as the `Idempotency-Key` header; the embedded
    /// transport hands it to `Engine::call_with_key`. A retry with the same key replays
    /// the first attempt's recorded response instead of running the write again.
    async fn call_with_key<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
        key: &str,
    ) -> Result<O, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned;
"#;

/// The streaming door, emitted only for a schema with a `-> stream` query.
pub(crate) const TRANSPORT_CALL_STREAM: &str = r#"
    /// Start a `-> stream` query and return its live row stream. An `Err` here means
    /// the call never started — a transport failure or a pre-body rejection carrying
    /// its real HTTP status; a failure after the stream begins arrives as the stream's
    /// final `Err` item. An HTTP transport feeds the NDJSON response body through
    /// [`decode_ndjson`]; the embedded transport yields the engine's rows in-process.
    async fn call_stream<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
    ) -> Result<RowStream<O>, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned + Send + 'static;
"#;

/// Closes the `Transport` trait and declares the client struct.
pub(crate) const TRANSPORT_TAIL: &str = r#"}

/// The generated client, generic over a `Transport`.
pub struct Client<T> {
    pub transport: T,
}
"#;
