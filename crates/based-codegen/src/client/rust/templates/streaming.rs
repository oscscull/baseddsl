/// The streaming surface, emitted only for a schema with a `-> stream` query: the
/// `RowStream` return type and the NDJSON decoder any HTTP transport feeds its
/// response body through. The decoder owns the framing contract (terminal line
/// mandatory, truncation = transport error), so every transport inherits it.
/// `futures_core` is referenced by full path — like `rust_decimal`, the consumer
/// needs the dependency only when the schema uses the feature.
pub(crate) const STREAMING: &str = r#"
// ---------- streaming ----------

/// A live row stream from a `-> stream` query, in sort order. Each item is one typed
/// row; an in-band server failure or a truncated body arrives as an `Err` item, and
/// after an `Err` item the stream is finished. **Drop = cancel**: dropping the stream
/// abandons the pass and releases its resources (the server gets its database
/// connection back).
pub type RowStream<O> =
    std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<O, ClientError>> + Send>>;

/// Decode an NDJSON response body (any stream of byte chunks) into the typed row
/// stream, enforcing the framing contract: one `{"row":…}` envelope per line, then
/// exactly one terminal line — `{"done":{"rows":N}}` on success (`rows` doubles as an
/// integrity checksum) or `{"error":{code,message}}` for an in-band failure (an `Err`
/// item carrying the server's stable code). A body that ends **without** a terminal
/// line was truncated (connection cut, server death) and yields a transport-kind
/// `Err`, never silent completion. An HTTP `Transport` feeds its response body
/// through this, so the framing rules live here once.
pub fn decode_ndjson<O, B, C, E>(body: B) -> RowStream<O>
where
    O: serde::de::DeserializeOwned + Send + 'static,
    B: futures_core::Stream<Item = Result<C, E>> + Send + 'static,
    C: AsRef<[u8]>,
    E: std::error::Error + Send + Sync + 'static,
{
    Box::pin(NdjsonRows {
        body: Box::pin(body),
        buf: Vec::new(),
        rows_seen: 0,
        body_done: false,
        finished: false,
        _row: PhantomData,
    })
}

/// The stream behind [`decode_ndjson`]: buffers body chunks, decodes each complete
/// line as one envelope, and tracks the terminal-line contract.
struct NdjsonRows<B, O> {
    body: std::pin::Pin<Box<B>>,
    buf: Vec<u8>,
    /// Rows yielded so far, checked against the terminal `done.rows` checksum.
    rows_seen: u64,
    /// The body ended (EOF). Reaching it before a terminal line is truncation.
    body_done: bool,
    /// A terminal line (or terminal `Err` item) was emitted; the stream is over.
    finished: bool,
    _row: PhantomData<fn() -> O>,
}

impl<B, C, E, O> futures_core::Stream for NdjsonRows<B, O>
where
    B: futures_core::Stream<Item = Result<C, E>>,
    C: AsRef<[u8]>,
    E: std::error::Error + Send + Sync + 'static,
    O: serde::de::DeserializeOwned,
{
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
        loop {
            // Decode every complete buffered line before touching the transport.
            while let Some(pos) = this.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = this.buf.drain(..=pos).collect();
                let line = &line[..line.len() - 1];
                if line.iter().all(|b| b.is_ascii_whitespace()) {
                    continue;
                }
                let envelope: serde_json::Value = match serde_json::from_slice(line) {
                    Ok(v) => v,
                    Err(e) => {
                        this.finished = true;
                        return Poll::Ready(Some(Err(ClientError::decode(e))));
                    }
                };
                if let Some(row) = envelope.get("row") {
                    this.rows_seen += 1;
                    return Poll::Ready(Some(match serde_json::from_value::<O>(row.clone()) {
                        Ok(row) => Ok(row),
                        Err(e) => {
                            this.finished = true;
                            Err(ClientError::decode(e))
                        }
                    }));
                }
                if let Some(done) = envelope.get("done") {
                    this.finished = true;
                    // `done.rows` is the integrity checksum: a disagreement means a
                    // row line was lost in transit — report it, never silent success.
                    let counted = done.get("rows").and_then(serde_json::Value::as_u64);
                    if counted != Some(this.rows_seen) {
                        return Poll::Ready(Some(Err(ClientError::transport(format!(
                            "stream checksum mismatch: server reports {counted:?} rows, received {}",
                            this.rows_seen
                        )))));
                    }
                    return Poll::Ready(None);
                }
                if let Some(error) = envelope.get("error") {
                    this.finished = true;
                    // The 200 status line was spent before the failure; 503 is the
                    // status the same database failure carries before the body.
                    let code = error["code"].as_str().unwrap_or("error");
                    let message = error["message"].as_str().unwrap_or("stream failed");
                    return Poll::Ready(Some(Err(ClientError::api(503, code, message))));
                }
                this.finished = true;
                return Poll::Ready(Some(Err(ClientError::transport(format!(
                    "unrecognized stream envelope: {envelope}"
                )))));
            }
            if this.body_done {
                // EOF without a terminal line (a partial line in the buffer counts):
                // the body was truncated. Never treat it as completion.
                this.finished = true;
                return Poll::Ready(Some(Err(ClientError::transport(
                    "response body ended without a terminal `done`/`error` line (truncated)"
                        .to_string(),
                ))));
            }
            match this.body.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => this.buf.extend_from_slice(chunk.as_ref()),
                Poll::Ready(Some(Err(e))) => {
                    this.finished = true;
                    return Poll::Ready(Some(Err(ClientError::transport(e))));
                }
                Poll::Ready(None) => this.body_done = true,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
"#;
