/// The opt-in in-process bridge, appended when [`ClientOptions::embedded`] is set. It
/// references `based_runtime::Engine` *by path* — the consuming crate depends on
/// based-runtime (based-codegen itself does not; that would be circular). This is the
/// bridge an embedder would otherwise hand-copy: serialize the typed input and `$ctx`
/// to JSON (a non-object ctx → `{}`), call `engine.call`, decode a `200` body into
/// `O`, map a non-`200` to a `ClientError` from `error.message`. Split so a schema
/// can add the keyed and streaming doors inside the same `impl` (head +
/// [`EMBEDDED_KEYED_CALL`] + [`EMBEDDED_STREAM_CALL`] + tail); head + tail alone
/// is the exact minimal bridge earlier versions emitted.
pub(crate) const EMBEDDED_HEAD: &str = r#"
// ---------- embedded bridge (based_runtime::Engine) ----------

/// A `Transport` backed by an in-process `based_runtime::Engine` — every callable runs
/// through the engine's dispatch core with no socket. Build one with [`embedded`].
pub struct Embedded<'a> {
    engine: &'a based_runtime::Engine,
}

impl Transport for Embedded<'_> {
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
        let resp = self.engine.call(route, args, ctx).await;
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(ClientError::decode)
        } else {
            // Preserve the server's structured error: its status + stable code + message.
            let code = resp.body["error"]["code"].as_str().unwrap_or("error");
            let message = resp.body["error"]["message"].as_str().unwrap_or("call failed");
            Err(ClientError::api(resp.status, code, message))
        }
    }
"#;

/// The embedded transport's keyed mutation door, spliced into the `impl Transport
/// for Embedded` block when the schema has a mutation.
pub(crate) const EMBEDDED_KEYED_CALL: &str = r#"
    /// The keyed door in-process: the same idempotent-replay contract the HTTP
    /// `Idempotency-Key` header gets, via `Engine::call_with_key`.
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
        O: serde::de::DeserializeOwned,
    {
        let args = serde_json::to_value(input).map_err(ClientError::decode)?;
        // `&()` → JSON `null`; the engine treats a non-object context as empty.
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { serde_json::json!({}) })
            .map_err(ClientError::decode)?;
        let resp = self
            .engine
            .call_with_key(route, args, ctx, Some(key.to_string()))
            .await;
        if resp.status == 200 {
            serde_json::from_value(resp.body).map_err(ClientError::decode)
        } else {
            // Preserve the server's structured error: its status + stable code + message.
            let code = resp.body["error"]["code"].as_str().unwrap_or("error");
            let message = resp.body["error"]["message"].as_str().unwrap_or("call failed");
            Err(ClientError::api(resp.status, code, message))
        }
    }
"#;

/// The embedded transport's streaming door, spliced into the `impl Transport for
/// Embedded` block when the schema has a `-> stream` query.
pub(crate) const EMBEDDED_STREAM_CALL: &str = r#"
    /// Start a `-> stream` query in-process: the engine's shaped row stream decoded
    /// into the typed shape — the same items the HTTP path yields, with no socket and
    /// no NDJSON round-trip. The stream owns its database connection; dropping it
    /// cancels the pass and returns the connection to the pool.
    async fn call_stream<I, C, O>(
        &self,
        route: &str,
        input: &I,
        ctx: &C,
    ) -> Result<RowStream<O>, ClientError>
    where
        I: Serialize + Sync,
        C: Serialize + Sync,
        O: serde::de::DeserializeOwned + Send + 'static,
    {
        let args = serde_json::to_value(input).map_err(ClientError::decode)?;
        // `&()` → JSON `null`; the engine treats a non-object context as empty.
        let ctx = serde_json::to_value(ctx)
            .map(|v| if v.is_object() { v } else { serde_json::json!({}) })
            .map_err(ClientError::decode)?;
        match self.engine.call_stream(route, args, ctx).await {
            Ok(rows) => Ok(Box::pin(EngineRows {
                inner: rows,
                finished: false,
                _row: PhantomData,
            })),
            // A pre-body rejection: the same status + stable code the wire would send.
            Err(resp) => {
                let code = resp.body["error"]["code"].as_str().unwrap_or("error");
                let message = resp.body["error"]["message"].as_str().unwrap_or("call failed");
                Err(ClientError::api(resp.status, code, message))
            }
        }
    }
"#;

/// Closes the `impl Transport for Embedded` block and adds the one-call
/// constructor. [`EMBEDDED_HEAD`] + this is the whole non-streaming bridge.
pub(crate) const EMBEDDED_TAIL: &str = r#"}

/// A ready-to-use client over an in-process `based_runtime::Engine` — no bridge to write.
/// `$ctx` is a typed per-call argument the app sets, not the caller; a public callable
/// passes `()`, which maps to an empty context bag.
pub fn embedded(engine: &based_runtime::Engine) -> Client<Embedded<'_>> {
    Client {
        transport: Embedded { engine },
    }
}
"#;

/// The embedded streaming adapter, appended after the bridge when the schema has a
/// `-> stream` query: decodes each engine row into the typed shape and maps a
/// mid-pass database failure to the same typed `Err` the wire's in-band `error`
/// line produces.
pub(crate) const EMBEDDED_ENGINE_ROWS: &str = r#"
/// The embedded transport's row stream: `based_runtime::ShapedStream` items decoded
/// into the typed shape. After an `Err` item the stream is finished (the engine's
/// stream already ends after its error; a decode failure ends this one).
struct EngineRows<O> {
    inner: based_runtime::ShapedStream,
    finished: bool,
    _row: PhantomData<fn() -> O>,
}

impl<O: serde::de::DeserializeOwned> futures_core::Stream for EngineRows<O> {
    type Item = Result<O, ClientError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(row))) => {
                Poll::Ready(Some(match serde_json::from_value::<O>(row) {
                    Ok(row) => Ok(row),
                    Err(e) => {
                        this.finished = true;
                        Err(ClientError::decode(e))
                    }
                }))
            }
            Poll::Ready(Some(Err(e))) => {
                this.finished = true;
                // A mid-pass database failure: the same stable code the wire's in-band
                // `error` line carries, with the 503 the failure maps to pre-body.
                Poll::Ready(Some(Err(ClientError::api(503, e.code(), e.message))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
"#;
