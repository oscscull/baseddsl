//! The HTTP listener (`based serve`) — the thin axum edge over [`crate::serve::dispatch`].
//!
//! The interesting logic lives in the pure dispatch core; this module only decodes the
//! socket into `dispatch`'s arguments and writes its [`WireResponse`] back. It is an
//! async tokio service: concurrency is bounded by the backend's connection pool (a
//! request past the pool's capacity waits at most the checkout timeout, then fails fast
//! as a `503`), so no separate worker ceiling is configured here.
//!
//! ## Per-request flow
//! 1. Decode the request line (`POST /q|m/<name>`), headers, and the (size-capped) JSON
//!    body (the argument object). A non-POST or unroutable path is rejected before a
//!    connection is borrowed ([`crate::serve::preflight`]).
//! 2. Derive `$ctx` from the headers via the pluggable [`ContextSource`] — never the
//!    body (a client cannot inject scope; request context is server-supplied out of band
//!    by the auth edge). The shard key is derived from the callable's resolved `@scope`
//!    owner field pulled out of `$ctx` — the same `@scope` that filters the row, so
//!    routing and row-visibility share one source of truth (an explicit
//!    `X-Based-Shard-Key` header can override it).
//! 3. Run [`dispatch`] with a fresh per-request [`UuidGen`]; dispatch checks a
//!    connection out of the [`Backend`] for that shard key. The edge is
//!    `Backend`-generic, so any dialect's backend drops in without a change here.
//! 4. Write `WireResponse.status` + JSON body back. A `-> stream` query diverges only
//!    here: [`dispatch_stream`] starts the row stream (pre-body failures keep their
//!    real statuses) and the body is NDJSON with a mandatory terminal line.
//!
//! ## Operational endpoints
//! Two unauthenticated `GET` probes an orchestrator / load balancer uses, answered before
//! any routing:
//! - `GET /healthz` — liveness: the process is up and serving. Always `200` while
//!   serving; a container that fails this is restarted. No DB touch.
//! - `GET /readyz` — readiness: this instance should receive traffic. `200` only when the
//!   backend can serve (`Backend::ping`) and we are not draining. On shutdown it flips to
//!   `503` first, so the load balancer pulls the instance out of rotation before in-flight
//!   requests finish — the drain half of a zero-downtime rolling deploy.
//!
//! ## Graceful shutdown
//! [`Handle::shutdown`] (wired to SIGTERM/SIGINT by the CLI) flips the drain flag (so
//! `/readyz` fails first), holds the listener open for [`DRAIN_WINDOW`] so the load
//! balancer's probe can observe the failing readiness and stop routing, then triggers
//! axum's graceful shutdown: the listener stops accepting, in-flight requests run to
//! completion, then [`serve`] returns.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::guard::Guards;
use crate::id::UuidGen;
use crate::idempotency::MemStore;
use crate::load::Compiled;
use crate::run::{Backend, ShapedStream};
use crate::serve::{
    dispatch, dispatch_stream, preflight, resolve_shard_key, route_target, WireResponse,
};

/// Largest request body we read (1 MiB). The wire carries a small argument object;
/// anything larger is malformed or hostile, so we cap the read rather than buffer an
/// unbounded body.
const MAX_BODY: usize = 1 << 20;

/// How long the listener keeps answering after `shutdown()` before it stops accepting.
/// Readiness must *observably* fail before the socket closes — a load balancer drains on
/// a 503 probe, not on connection refused — so the drain window is the probe's chance to
/// see it. In-flight requests are never cut off regardless; this only delays the close.
const DRAIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

/// Listener configuration: where to bind.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub listen: String,
}

/// Derives the request `$ctx` and shard key from a request's headers — the seam
/// between the transport and the auth edge. `$ctx` is server-supplied, never the body:
/// a real deployment fronts `based serve` with an auth proxy that authenticates the
/// caller and sets these headers (stripping any client-sent copy). The implementation
/// is pluggable so that policy lives outside the runtime.
pub trait ContextSource: Send + Sync {
    /// Return the derived context, or a [`WireResponse`] to write back instead (e.g. a
    /// `401` when required auth is absent). The default [`TrustedHeaderContext`] reads
    /// pre-authenticated headers.
    fn derive(&self, headers: &HeaderView) -> Result<Context, WireResponse>;
}

/// What a [`ContextSource`] produces: the request `$ctx` (passed to `dispatch` as the
/// out-of-band context) and an **optional explicit** shard-key override.
///
/// The shard key is normally derived from the schema: the callable's target model's
/// resolved `@scope` owner field, pulled out of `$ctx` by the listener — so the shard a
/// row lives in and the shard its owner's requests route to share one source of truth,
/// never a hand-set config. `shard_key_override` is the escape hatch: a deployment (or a
/// callable with no `@scope`) that must route by some other key sets the
/// `X-Based-Shard-Key` header, and it wins over the schema-derived field.
#[derive(Debug, Clone)]
pub struct Context {
    pub ctx: serde_json::Value,
    /// An explicit shard key from `X-Based-Shard-Key`, or `None` to let the listener
    /// derive it from the callable's `@scope` field.
    pub shard_key_override: Option<String>,
}

/// A case-insensitive view over the request's headers, handed to a [`ContextSource`].
pub struct HeaderView<'a>(&'a [(String, String)]);

impl HeaderView<'_> {
    /// The value of the first header named `name` (case-insensitive), if present.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The default context source: trust pre-authenticated headers set by an upstream
/// auth proxy (the standard deployment — the proxy authenticates, this reads).
///
/// - `$ctx` comes from the `X-Based-Context` header as a JSON object (absent → empty,
///   which is correct for a callable that requires no `$ctx`). Present-but-invalid or
///   non-object → `400` (a misconfigured edge, surfaced loudly rather than silently
///   dropped).
/// - The shard key is normally not read here — the listener derives it from the
///   callable's `@scope` field. This source only surfaces the `X-Based-Shard-Key` header
///   as an explicit override (usually absent), which the listener honours over the
///   schema-derived field.
///
/// This is the trusted-edge seam, not an authenticator: it assumes the proxy strips
/// any client-supplied `X-Based-*` header. For local development you set the headers
/// yourself.
#[derive(Default)]
pub struct TrustedHeaderContext;

impl ContextSource for TrustedHeaderContext {
    fn derive(&self, headers: &HeaderView) -> Result<Context, WireResponse> {
        let ctx = match headers.get("X-Based-Context") {
            None => serde_json::json!({}),
            Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
                Ok(v) if v.is_object() => v,
                _ => return Err(EdgeError::BadContext.into()),
            },
        };
        // The shard key is schema-derived unless a deployment forces it with an explicit
        // header. Only that override is read here; the derivation needs the route, which
        // the listener knows.
        let shard_key_override = headers.get("X-Based-Shard-Key").map(str::to_string);
        Ok(Context {
            ctx,
            shard_key_override,
        })
    }
}

/// Everything a request handler needs, shared read-only across all of them.
struct Shared {
    compiled: Compiled,
    backend: Box<dyn Backend>,
    ctx_source: Box<dyn ContextSource>,
    /// The mutation idempotency store, shared across all requests so a retry that lands
    /// on any task dedupes. `MemStore` dedupes within this one process; a
    /// multi-instance deployment wants a shared/durable store behind the same
    /// `IdempotencyStore` trait.
    idempotency: MemStore,
    /// Set once when a graceful shutdown is requested (SIGTERM/SIGINT). `/readyz` reads
    /// it to fail readiness first (drain).
    draining: Arc<AtomicBool>,
    /// Always empty: guards are host functions, and the standalone listener has no host
    /// code to register — startup refuses a guarded schema. Held so dispatch takes the
    /// one registry shape on every door.
    guards: Guards,
}

/// A control handle for a running listener, returned via [`serve_with_handle`]'s
/// `on_start`. Its one job is [`shutdown`](Handle::shutdown): request a graceful drain
/// from another thread (typically a signal handler). Cheap to clone — every clone
/// drives the same server.
#[derive(Clone)]
pub struct Handle {
    draining: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Handle {
    /// Begin a graceful shutdown: flip readiness to failing (so the load balancer drains
    /// this instance) and stop accepting new requests; in-flight requests run to
    /// completion, then the [`serve`]/[`serve_with_handle`] call returns. Idempotent —
    /// calling it twice is harmless.
    pub fn shutdown(&self) {
        self.draining.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        // A late waiter must still see the flag (notify_waiters wakes only current ones).
        self.notify.notify_one();
    }
}

/// Failure to *start* serving (bind the socket). Once serving, per-request failures
/// are [`WireResponse`]s, never errors out of here.
#[derive(Debug)]
pub struct ServeError(pub String);

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ServeError {}

/// A pre-dispatch failure the listener edge itself produces — a bad body, a malformed
/// `$ctx` header, a drain/readiness refusal — at the transport edge, before a request
/// reaches [`dispatch`]. The transport-edge twin of the core's [`crate::plan::PlanError`] /
/// [`crate::run::DbError`]: each carries a stable machine [`code`](EdgeError::code) and an
/// HTTP [`status`](EdgeError::status), so the edge's own wire codes live in one registry.
/// This registry covers the edge's own failures; operational DB failures the edge surfaces
/// (a checkout or ping fault) carry their own [`crate::run::DbError::code`].
#[derive(Debug, Clone, PartialEq)]
enum EdgeError {
    /// The `X-Based-Context` header held a non-object JSON value.
    BadContext,
    /// The request body was invalid UTF-8, a non-object JSON value, or unparseable JSON. The
    /// carried string is the specific reason.
    BadBody(String),
    /// This instance is draining (graceful shutdown); readiness fails first so the load
    /// balancer stops routing while in-flight requests finish.
    Draining,
    /// The backend readiness probe ([`Backend::ping`]) failed. The carried string is the
    /// driver's message.
    NotReady(String),
}

impl EdgeError {
    /// The stable, machine-readable code for this edge failure — the single source of
    /// truth for its wire `error.code`, the same convention the dispatch core's errors
    /// follow. Stable across releases.
    fn code(&self) -> &'static str {
        match self {
            EdgeError::BadContext => "bad_context",
            EdgeError::BadBody(_) => "bad_body",
            EdgeError::Draining => "draining",
            EdgeError::NotReady(_) => "not_ready",
        }
    }

    /// The HTTP status this edge failure maps to: a malformed request the caller can fix
    /// is `400`; a drain/readiness refusal is a retryable `503`.
    fn status(&self) -> u16 {
        match self {
            EdgeError::BadContext | EdgeError::BadBody(_) => 400,
            EdgeError::Draining | EdgeError::NotReady(_) => 503,
        }
    }
}

impl std::fmt::Display for EdgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdgeError::BadContext => f.write_str("X-Based-Context is not a JSON object"),
            EdgeError::BadBody(reason) => f.write_str(reason),
            EdgeError::Draining => f.write_str("server is shutting down"),
            EdgeError::NotReady(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EdgeError {}

impl From<EdgeError> for WireResponse {
    fn from(e: EdgeError) -> WireResponse {
        WireResponse::error(e.status(), e.code(), e.to_string())
    }
}

/// Render a [`WireResponse`] as the axum response: its status + JSON body.
fn into_response(resp: WireResponse) -> Response {
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, axum::Json(resp.body)).into_response()
}

/// Bind `config.listen` and serve requests until the process is killed. A caller that
/// wants graceful shutdown uses [`serve_with_handle`] instead and triggers the returned
/// [`Handle`] from a signal.
pub async fn serve(
    compiled: Compiled,
    backend: impl Backend + 'static,
    ctx_source: impl ContextSource + 'static,
    config: ServeConfig,
) -> Result<(), ServeError> {
    serve_with_handle(compiled, backend, ctx_source, config, |_| {}).await
}

/// Like [`serve`], but hands the caller a [`Handle`] (via `on_start`) *before* the
/// listener blocks, so it can wire the handle to a signal for graceful shutdown. The
/// `on_start` closure runs once, right after the socket binds — the point at which the
/// listener is accepting requests. This call resolves once every in-flight request has
/// finished after [`Handle::shutdown`].
pub async fn serve_with_handle(
    compiled: Compiled,
    backend: impl Backend + 'static,
    ctx_source: impl ContextSource + 'static,
    config: ServeConfig,
    on_start: impl FnOnce(Handle),
) -> Result<(), ServeError> {
    // A guard is a host function only an embedding app can register; this listener has
    // no host code, so a guarded schema must not come up here — refusing at startup is
    // what keeps a declared check from silently not running.
    if let Some((m, g)) = compiled.declared_guards().next() {
        return Err(ServeError(format!(
            "mutation `{m}` declares guard `{g}` — guards are host functions this listener \
             cannot register; embed the engine (Engine::with_guards) instead"
        )));
    }
    let draining = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        compiled,
        backend: Box::new(backend),
        ctx_source: Box::new(ctx_source),
        idempotency: MemStore::new(),
        draining: Arc::clone(&draining),
        guards: Guards::new(),
    });

    let app = Router::new()
        .route("/healthz", get(health_response))
        .route("/readyz", get(ready_response))
        // Every other path is the RPC surface; the fallback keeps the dispatch core's
        // own routing (and its 404/405 wire contract) as the one source of truth.
        .fallback(handle)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
        .with_state(Arc::clone(&shared));

    let listener = tokio::net::TcpListener::bind(&config.listen)
        .await
        .map_err(|e| ServeError(format!("cannot bind {}: {e}", config.listen)))?;

    let notify = Arc::new(tokio::sync::Notify::new());
    let handle = Handle {
        draining: Arc::clone(&draining),
        notify: Arc::clone(&notify),
    };
    // The listener is up: hand the caller its shutdown handle.
    on_start(handle);

    let drain = {
        let draining = Arc::clone(&draining);
        async move {
            // Wake on shutdown(); the flag check covers a shutdown that raced the await.
            while !draining.load(Ordering::SeqCst) {
                notify.notified().await;
            }
            // Readiness now fails; keep accepting until the LB has had a chance to see it.
            tokio::time::sleep(DRAIN_WINDOW).await;
        }
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(drain)
        .await
        .map_err(|e| ServeError(format!("serve failed: {e}")))
}

/// Decode one request, run it through the dispatch core, and write the response. A
/// `-> stream` query takes the streaming path: the same pre-body decode + validation
/// (failures keep their real HTTP statuses), then the NDJSON body; every other request
/// is the buffered `dispatch` → JSON response, unchanged.
async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let d = match decode_request(&shared, &method, &uri, &headers, &body) {
        Ok(d) => d,
        Err(resp) => return into_response(resp),
    };

    if !d.is_mutation && shared.compiled.is_stream_query(&d.name) {
        return match dispatch_stream(
            &shared.compiled,
            &*shared.backend,
            &d.shard_key,
            method.as_str(),
            uri.path(),
            d.args,
            d.ctx,
        )
        .await
        {
            Ok(rows) => ndjson_response(rows),
            Err(resp) => into_response(resp),
        };
    }

    let mut id_gen = UuidGen;
    into_response(
        dispatch(
            &shared.compiled,
            &*shared.backend,
            &d.shard_key,
            &mut id_gen,
            &shared.idempotency,
            &shared.guards,
            method.as_str(),
            uri.path(),
            d.args,
            d.ctx,
            d.idem_key,
        )
        .await,
    )
}

/// A request decoded to `dispatch`'s arguments — everything the edge derives before
/// any connection is borrowed.
struct Decoded {
    is_mutation: bool,
    name: String,
    shard_key: String,
    args: serde_json::Value,
    ctx: serde_json::Value,
    idem_key: Option<String>,
}

/// The pure pre-dispatch half of a request: route → derive `$ctx` → resolve the shard
/// key → parse the body. Every failure is a [`WireResponse`], so the handler never
/// branches on kinds.
fn decode_request(
    shared: &Shared,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Decoded, WireResponse> {
    let path = uri.path();

    // Reject a non-POST/unroutable request before borrowing a connection.
    if let Some(resp) = preflight(method.as_str(), path) {
        return Err(resp);
    }
    // Preflight guaranteed a routable path, so this is the callable to run — needed now
    // to derive the shard key from its `@scope` field, before checkout.
    let (is_mutation, name) = route_target(path).expect("preflight guaranteed a routable path");

    // Derive $ctx (never the body) + the explicit shard-key override, from the headers.
    let header_pairs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let header_view = HeaderView(&header_pairs);
    let context = shared.ctx_source.derive(&header_view)?;

    // The shard key: the callable's `@scope` owner field pulled out of `$ctx`, or an
    // explicit header override, or "" (unscoped / single-shard → shard 0). Derived from
    // the same `@scope` that filters the row, so routing and row-visibility can't drift.
    let shard_key = resolve_shard_key(
        &shared.compiled,
        is_mutation,
        name,
        &context.ctx,
        context.shard_key_override.as_deref(),
    );

    // The mutation idempotency key rides the standard `Idempotency-Key` header — out of
    // band, never the body. Absent/blank → no dedupe; queries ignore it.
    let idem_key = header_view.get("Idempotency-Key").map(str::to_string);

    // Decode the JSON argument object from the (size-capped) body.
    let args = read_json_body(body)?;

    Ok(Decoded {
        is_mutation,
        name: name.to_string(),
        shard_key,
        args,
        ctx: context.ctx,
        idem_key,
    })
}

/// The `-> stream` response: `200` + `application/x-ndjson`, one envelope object per
/// line. The status line is written before the first row, so it is spent by the time a
/// late failure can happen — the terminal line is the in-band verdict.
fn ndjson_response(rows: ShapedStream) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/x-ndjson")
        .body(axum::body::Body::from_stream(ndjson_lines(rows)))
        .expect("static response parts are valid")
}

/// Frame a [`ShapedStream`] as NDJSON lines: `{"row":…}` per row, then exactly one
/// terminal line — `{"done":{"rows":N}}` on success or `{"error":{code,message}}` on a
/// mid-stream failure. A body that ends without a terminal line was truncated
/// (connection cut, server death) and the client must treat it as a transport error;
/// `done.rows` doubles as an integrity checksum. A client disconnect drops this
/// stream, which drops the row stream and its connection — cancel, not a leak.
fn ndjson_lines(
    rows: ShapedStream,
) -> impl futures_core::Stream<Item = Result<Bytes, std::convert::Infallible>> {
    use futures_util::StreamExt;
    async_stream::stream! {
        let mut rows = rows;
        let mut count: u64 = 0;
        while let Some(item) = rows.next().await {
            match item {
                Ok(row) => {
                    count += 1;
                    yield Ok(ndjson_line(serde_json::json!({ "row": row })));
                }
                // The stream is finished after an error: the error line is terminal.
                Err(e) => {
                    yield Ok(ndjson_line(
                        serde_json::json!({ "error": { "code": e.code(), "message": e.message } }),
                    ));
                    return;
                }
            }
        }
        yield Ok(ndjson_line(serde_json::json!({ "done": { "rows": count } })));
    }
}

/// One NDJSON line: the envelope object, compact-serialized, newline-terminated.
fn ndjson_line(envelope: serde_json::Value) -> Bytes {
    let mut s = envelope.to_string();
    s.push('\n');
    Bytes::from(s)
}

/// Liveness (`GET /healthz`): the process is up and serving. Always `200` while a
/// task can answer — reaching this code *is* the liveness signal. It deliberately does
/// **not** consult the backend: a DB outage must not restart an otherwise-healthy app
/// container (that is readiness's job — drain, don't restart). While draining it still
/// reports live (the process is up until the last request finishes); readiness is what
/// flips first.
async fn health_response(State(_shared): State<Arc<Shared>>) -> Response {
    into_response(WireResponse::ok(serde_json::json!({ "status": "ok" })))
}

/// Readiness (`GET /readyz`): should this instance receive traffic *now*? `200` only when
/// (a) we are not draining and (b) the backend can serve ([`Backend::ping`]). A `503`
/// with `{ error: { code, message } }` otherwise — the load balancer pulls the instance
/// out of rotation on that, which is exactly the drain (shutdown) and back-pressure (DB
/// down) behaviour we want. Distinct from liveness so a transient DB blip drains rather
/// than restarts the container.
async fn ready_response(State(shared): State<Arc<Shared>>) -> Response {
    if shared.draining.load(Ordering::SeqCst) {
        // The drain half of a zero-downtime rollout: fail readiness first so the LB stops
        // sending new requests while in-flight ones finish.
        return into_response(EdgeError::Draining.into());
    }
    into_response(match shared.backend.ping().await {
        Ok(()) => WireResponse::ok(serde_json::json!({ "status": "ready" })),
        Err(e) => EdgeError::NotReady(e.message).into(),
    })
}

/// Parse the request body as the JSON argument object. An empty body is an empty object
/// (a no-arg callable). A non-object or unparseable body is a `400` — a client mistake
/// it can fix.
fn read_json_body(body: &Bytes) -> Result<serde_json::Value, EdgeError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| EdgeError::BadBody("request body is not valid UTF-8".to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(EdgeError::BadBody(
            "request body must be a JSON object".to_string(),
        )),
        Err(e) => Err(EdgeError::BadBody(format!("invalid JSON body: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::resolve_shard_key;
    use serde_json::json;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn header_view_is_case_insensitive() {
        let h = headers(&[("X-Based-Shard-Key", "org-7")]);
        let view = HeaderView(&h);
        assert_eq!(view.get("x-based-shard-key"), Some("org-7"));
        assert_eq!(view.get("X-BASED-SHARD-KEY"), Some("org-7"));
        assert_eq!(view.get("missing"), None);
    }

    #[test]
    fn trusted_header_context_parses_ctx_and_explicit_key() {
        let src = TrustedHeaderContext;
        let h = headers(&[
            ("X-Based-Context", r#"{"org":"org-1"}"#),
            ("X-Based-Shard-Key", "shard-9"),
        ]);
        let c = src.derive(&HeaderView(&h)).unwrap();
        assert_eq!(c.ctx, serde_json::json!({ "org": "org-1" }));
        // The explicit key header is surfaced as the override (it wins at resolution).
        assert_eq!(c.shard_key_override.as_deref(), Some("shard-9"));
    }

    #[test]
    fn absent_context_is_empty_and_unkeyed() {
        let src = TrustedHeaderContext;
        let c = src.derive(&HeaderView(&[])).unwrap();
        assert_eq!(c.ctx, serde_json::json!({}));
        assert_eq!(c.shard_key_override, None);
    }

    #[test]
    fn non_object_context_is_rejected() {
        let src = TrustedHeaderContext;
        let h = headers(&[("X-Based-Context", "[1,2,3]")]);
        let err = src.derive(&HeaderView(&h)).unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.body["error"]["code"], "bad_context");
    }

    #[test]
    fn edge_error_registry_maps_code_status_and_message() {
        // The edge's own failures carry a stable code + status through the registry, and
        // the `WireResponse` envelope is built from them (one source of truth for the wire).
        for (err, code, status) in [
            (EdgeError::BadContext, "bad_context", 400),
            (EdgeError::BadBody("nope".into()), "bad_body", 400),
            (EdgeError::Draining, "draining", 503),
            (EdgeError::NotReady("db down".into()), "not_ready", 503),
        ] {
            assert_eq!(err.code(), code);
            assert_eq!(err.status(), status);
            let resp: WireResponse = err.clone().into();
            assert_eq!(resp.status, status);
            assert_eq!(resp.body["error"]["code"], code);
            assert_eq!(resp.body["error"]["message"], err.to_string());
        }
    }

    // ---- shard-key derivation from `@scope`  ----------------------------

    /// A tiny scoped schema: `Order @scope Tenant`, one scoped query, one scoped
    /// mutation, one `unscoped` cross-org query, and one unscoped-model query.
    fn compiled() -> crate::load::Compiled {
        use based_ast::FileId;
        const SCHEMA: &str = r#"
            Org { name: text }
            scope Tenant (org: Org = $ctx.org)
            @scope Tenant
            Order { org: Org, status: text }
            shape OrderCard from Order { status }

            query order_by_id(id) -> OrderCard scoped Tenant;
            query all_orders(org) -> OrderCard[] unscoped("admin");
            query list_orgs() -> Org[] { list Org; }
            mutation place_order(status) -> OrderCard scoped Tenant {
                create Order { status = $status };
            }
        "#;
        let sf = based_parser::parse_file(SCHEMA, FileId(0)).expect("parse");
        let (schema, diags) = based_sema::check(&sf.decls);
        assert!(
            !diags
                .iter()
                .any(|d| d.severity == based_diagnostics::Severity::Error),
            "schema should check clean: {diags:?}"
        );
        crate::load::Compiled::from_checked(schema, sf.decls, based_codegen::Dialect::MariaDb)
    }

    #[test]
    fn scoped_query_shards_on_its_scope_ctx_field() {
        let c = compiled();
        // `Order @scope(org = $ctx.org)` → a query on it shards on `$ctx.org`.
        assert_eq!(c.shard_key_field(false, "order_by_id"), Some("org"));
        let key = resolve_shard_key(&c, false, "order_by_id", &json!({ "org": "org-1" }), None);
        assert_eq!(key, "org-1");
    }

    #[test]
    fn scoped_mutation_shards_on_its_scope_ctx_field() {
        let c = compiled();
        assert_eq!(c.shard_key_field(true, "place_order"), Some("org"));
        let key = resolve_shard_key(&c, true, "place_order", &json!({ "org": "org-9" }), None);
        assert_eq!(key, "org-9");
    }

    #[test]
    fn unscoped_callable_has_no_shard_field() {
        let c = compiled();
        // `unscoped("admin")` disables scope → no owning shard → key "" (shard 0).
        assert_eq!(c.shard_key_field(false, "all_orders"), None);
        let key = resolve_shard_key(&c, false, "all_orders", &json!({ "org": "org-1" }), None);
        assert_eq!(key, "");
    }

    #[test]
    fn unscoped_model_has_no_shard_field() {
        let c = compiled();
        // `Org` has no `@scope`, so a query on it has no shard field.
        assert_eq!(c.shard_key_field(false, "list_orgs"), None);
        let key = resolve_shard_key(&c, false, "list_orgs", &json!({}), None);
        assert_eq!(key, "");
    }

    #[test]
    fn explicit_header_overrides_scope_field() {
        let c = compiled();
        // The override wins even for a scoped callable.
        let key = resolve_shard_key(
            &c,
            false,
            "order_by_id",
            &json!({ "org": "org-1" }),
            Some("forced-shard"),
        );
        assert_eq!(key, "forced-shard");
    }

    #[test]
    fn non_string_scope_value_is_stringified() {
        // A tenant id that arrives as a JSON number still yields a stable byte key.
        let c = compiled();
        let key = resolve_shard_key(&c, false, "order_by_id", &json!({ "org": 42 }), None);
        assert_eq!(key, "42");
    }

    #[test]
    fn missing_scope_value_in_ctx_is_empty_key() {
        // The callable is scoped but `$ctx` lacks the field (a plan error follows in
        // dispatch — here we only pin the routing: no owner → shard 0).
        let c = compiled();
        let key = resolve_shard_key(&c, false, "order_by_id", &json!({}), None);
        assert_eq!(key, "");
    }
}
