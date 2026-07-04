//! The HTTP listener (`based serve`) — the thin socket edge over [`crate::serve::dispatch`].
//!
//! Everything interesting already lives in the pure dispatch core; this module only
//! decodes the socket into `dispatch`'s arguments and writes its [`WireResponse`] back.
//! Per D20 it is a **sync, bounded worker-thread pool** over the driver's **bounded
//! connection pool**: `workers` threads share one blocking [`tiny_http::Server`], each
//! looping `recv → handle → respond`. Capacity is added by adding shards + app
//! instances behind a load balancer, not threads-per-process — so the thread count is
//! bounded and matched to the pool, never unbounded.
//!
//! ## Per-request flow
//! 1. Decode the request line (`POST /q|m/<name>`), headers, and the JSON body (the
//!    argument object — calling.md #2). A non-POST or unroutable path is rejected
//!    *before* a connection is borrowed ([`crate::serve::preflight`]).
//! 2. Derive `$ctx` + the shard key from the headers via the pluggable
//!    [`ContextSource`] — **never the body** (auth.md, D7: a client cannot inject scope;
//!    request context is server-supplied out of band by the auth edge).
//! 3. Check a connection out of the [`Backend`] for that shard key (single-shard
//!    dispatch, D20) and run [`dispatch`] with a fresh per-request [`UuidGen`]. The
//!    edge is `Backend`-generic — it never names a concrete driver — so a Postgres /
//!    MySQL / SQLite backend drops in without a change here.
//! 4. Write `WireResponse.status` + JSON body back. A pool checkout failure is a
//!    [`crate::run::DbError`] → a retryable `503`, exactly like an in-flight DB fault.
//!
//! ## Operational endpoints (the container story)
//! Two unauthenticated `GET` probes an orchestrator / load balancer uses, answered
//! before any routing so they never touch a database connection except where readiness
//! deliberately does:
//! - `GET /healthz` — **liveness**: the process is up and its worker loop is running.
//!   Always `200` while serving; a container that fails this is restarted. No DB touch.
//! - `GET /readyz` — **readiness**: this instance should receive traffic. `200` only when
//!   the backend can serve (`Backend::ping`) *and* we are not draining. On shutdown it
//!   flips to `503` first, so the load balancer pulls the instance out of rotation before
//!   in-flight requests finish — the drain half of a zero-downtime rolling deploy.
//!
//! ## Graceful shutdown
//! [`Handle::shutdown`] (wired to SIGTERM/SIGINT by the CLI) flips a shared flag: workers
//! poll it between requests (via `recv_timeout`, so a blocked worker wakes on its own),
//! stop accepting new work, and exit once their *current* request finishes — in-flight
//! requests always run to completion, none is cut off mid-response.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::id::UuidGen;
use crate::idempotency::MemStore;
use crate::load::Compiled;
use crate::run::Backend;
use crate::serve::{dispatch, preflight, WireResponse};

/// Largest request body we read (1 MiB). The wire carries a small argument object;
/// anything larger is malformed or hostile, so we cap the read rather than let a
/// worker buffer an unbounded body.
const MAX_BODY: u64 = 1 << 20;

/// Listener configuration: where to bind and how many worker threads to run. The
/// worker count is the per-process concurrency ceiling; keep it in step with the
/// router's total pool capacity (workers past the available connections just block on
/// checkout).
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub listen: String,
    pub workers: usize,
}

/// Derives the request `$ctx` and shard key from a request's headers — the seam
/// between the transport and the auth edge. `$ctx` is **server-supplied, never the
/// body** (auth.md, D7): a real deployment fronts `based serve` with an auth proxy
/// that authenticates the caller and sets these headers (stripping any client-sent
/// copy). The implementation is pluggable so that policy lives outside the runtime.
pub trait ContextSource: Send + Sync {
    /// Return the derived context, or a [`WireResponse`] to write back instead (e.g. a
    /// `401` when required auth is absent). The default [`TrustedHeaderContext`] reads
    /// pre-authenticated headers.
    fn derive(&self, headers: &HeaderView) -> Result<Context, WireResponse>;
}

/// What a [`ContextSource`] produces: the request `$ctx` (passed to `dispatch` as the
/// out-of-band context) and the shard key that routes the request to one physical
/// shard (D20 — the key source is deliberately decoupled from `@scope`/D19).
#[derive(Debug, Clone)]
pub struct Context {
    pub ctx: serde_json::Value,
    pub shard_key: String,
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
/// - The shard key comes from the `X-Based-Shard-Key` header, or — absent that — the
///   configured `$ctx` field (typically the tenant/owner). Absent everywhere it is the
///   empty string, which the single-shard router (the common case) sends to shard 0.
///
/// This is the trusted-edge seam, not an authenticator: it assumes the proxy strips
/// any client-supplied `X-Based-*` header. For local development you set the headers
/// yourself.
#[derive(Default)]
pub struct TrustedHeaderContext {
    /// The `$ctx` field to fall back to for the shard key when no explicit
    /// `X-Based-Shard-Key` header is sent (e.g. `"org"`). `None` → key is header-only.
    pub shard_key_field: Option<String>,
}

impl ContextSource for TrustedHeaderContext {
    fn derive(&self, headers: &HeaderView) -> Result<Context, WireResponse> {
        let ctx = match headers.get("X-Based-Context") {
            None => serde_json::json!({}),
            Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
                Ok(v) if v.is_object() => v,
                _ => {
                    return Err(WireResponse::error(
                        400,
                        "bad_context",
                        "X-Based-Context is not a JSON object".to_string(),
                    ))
                }
            },
        };
        // Shard key: explicit header wins, else the configured $ctx field, else "".
        let shard_key = headers
            .get("X-Based-Shard-Key")
            .map(str::to_string)
            .or_else(|| {
                let field = self.shard_key_field.as_deref()?;
                match ctx.get(field)? {
                    serde_json::Value::String(s) => Some(s.clone()),
                    other => Some(other.to_string()),
                }
            })
            .unwrap_or_default();
        Ok(Context { ctx, shard_key })
    }
}

/// Everything a worker thread needs, shared read-only across all of them.
struct Shared {
    compiled: Compiled,
    backend: Box<dyn Backend>,
    ctx_source: Box<dyn ContextSource>,
    /// The mutation idempotency store (D25), shared across all workers so a retry that
    /// lands on any worker dedupes. `MemStore` dedupes within this one process; a
    /// multi-instance deployment wants a shared/durable store behind the same seam
    /// (deferred to the live-DB slice — the `IdempotencyStore` trait is identical).
    idempotency: MemStore,
    /// Set once when a graceful shutdown is requested (SIGTERM/SIGINT). `/readyz` reads
    /// it to fail readiness first (drain), and the worker loop reads it to stop.
    draining: Arc<AtomicBool>,
}

/// A control handle for a running listener, returned by [`serve_with_handle`]. Its one
/// job is [`shutdown`](Handle::shutdown): request a graceful drain from another thread
/// (typically a signal handler). Cheap to clone — every clone drives the same server.
#[derive(Clone)]
pub struct Handle {
    draining: Arc<AtomicBool>,
    server: Arc<Server>,
}

impl Handle {
    /// Begin a graceful shutdown: flip readiness to failing (so the load balancer drains
    /// this instance) and tell the workers to stop after their current request. Returns
    /// immediately; the [`serve`]/[`serve_with_handle`] call returns once every worker
    /// has finished its in-flight request and exited. Idempotent — calling it twice is
    /// harmless.
    pub fn shutdown(&self) {
        self.draining.store(true, Ordering::SeqCst);
        // Wake any worker blocked in `recv_timeout` so it observes the flag now rather
        // than after its poll interval elapses (faster drain; `unblock` wakes one, the
        // short poll timeout covers the rest).
        self.server.unblock();
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

/// How long a worker blocks on one `recv` before waking to re-check the drain flag.
/// Short enough that shutdown is prompt even for a worker that `unblock` didn't wake,
/// long enough that idle polling is negligible.
const DRAIN_POLL: Duration = Duration::from_millis(100);

/// Bind `config.listen` and serve requests until the process is killed. Spawns
/// `config.workers` threads over one shared blocking server (D20: a bounded
/// worker-thread pool, not a thread-per-connection). Blocks the calling thread until
/// every worker returns. A caller that wants **graceful shutdown** uses
/// [`serve_with_handle`] instead and triggers the returned [`Handle`] from a signal.
pub fn serve(
    compiled: Compiled,
    backend: impl Backend + 'static,
    ctx_source: impl ContextSource + 'static,
    config: ServeConfig,
) -> Result<(), ServeError> {
    // No handle wanted here: build one, drop it, and run until the process is killed.
    serve_with_handle(compiled, backend, ctx_source, config, |_| {})
}

/// Like [`serve`], but hands the caller a [`Handle`] (via `on_start`) *before* the
/// listener blocks, so it can wire the handle to a signal for graceful shutdown. The
/// `on_start` closure runs once, on the serving thread, right after the socket binds
/// and the workers spawn — the point at which the listener is accepting requests. This
/// call blocks until every worker has drained and exited (i.e. after [`Handle::shutdown`]).
pub fn serve_with_handle(
    compiled: Compiled,
    backend: impl Backend + 'static,
    ctx_source: impl ContextSource + 'static,
    config: ServeConfig,
    on_start: impl FnOnce(Handle),
) -> Result<(), ServeError> {
    let server = Server::http(&config.listen)
        .map_err(|e| ServeError(format!("cannot bind {}: {e}", config.listen)))?;
    let server = Arc::new(server);
    let draining = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        compiled,
        backend: Box::new(backend),
        ctx_source: Box::new(ctx_source),
        idempotency: MemStore::new(),
        draining: Arc::clone(&draining),
    });

    let workers = config.workers.max(1);
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let server = Arc::clone(&server);
        let shared = Arc::clone(&shared);
        handles.push(thread::spawn(move || worker_loop(&server, &shared)));
    }

    // The listener is up: hand the caller its shutdown handle.
    on_start(Handle {
        draining,
        server: Arc::clone(&server),
    });

    for h in handles {
        // A worker ends when the drain flag is set (graceful) or the server is dropped;
        // a panicked worker is logged by the default hook. Joining blocks the serving
        // thread until every worker's in-flight request has finished.
        let _ = h.join();
    }
    Ok(())
}

/// One worker: pull the next request off the shared server and handle it, until a
/// graceful shutdown is requested. Between requests it polls the drain flag (waking from
/// its blocking wait every [`DRAIN_POLL`]); once set, the loop exits after the *current*
/// request completes — no in-flight request is ever cut off (the drain guarantee).
fn worker_loop(server: &Server, shared: &Shared) {
    loop {
        if shared.draining.load(Ordering::SeqCst) {
            return;
        }
        match server.recv_timeout(DRAIN_POLL) {
            // A request arrived: run it to completion before re-checking the drain flag.
            Ok(Some(request)) => handle(request, shared),
            // The poll interval elapsed with no request (or `unblock` woke us): loop back
            // and re-check the drain flag.
            Ok(None) => {}
            // The server was dropped (or a socket error): nothing more to serve.
            Err(_) => return,
        }
    }
}

/// Decode one request, run it, and write the response. All the branching lives in
/// [`build_response`]; this only owns the socket read/write.
fn handle(mut request: Request, shared: &Shared) {
    let response = build_response(&mut request, shared);
    let body = response.body.to_string();
    let json = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static header is valid");
    let out = Response::from_string(body)
        .with_status_code(response.status)
        .with_header(json);
    // A write failure means the client hung up; nothing to do but drop it.
    let _ = request.respond(out);
}

/// The pure heart of a request: route → derive `$ctx` → check out a shard connection →
/// `dispatch`. Every failure is a [`WireResponse`], so `handle` never branches on kinds.
fn build_response(request: &mut Request, shared: &Shared) -> WireResponse {
    let is_get = matches!(request.method(), Method::Get);
    let method = request.method().as_str().to_string();
    // `url()` is path + optional query; our routes carry no query, so drop it.
    let path = request
        .url()
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_string();

    // Operational probes (liveness/readiness) are unauthenticated GETs answered before
    // routing — they carry no body, no `$ctx`, and (except readiness's ping) touch no DB.
    if is_get {
        match path.as_str() {
            "/healthz" => return health_response(shared),
            "/readyz" => return ready_response(shared),
            _ => {}
        }
    }

    // Reject a non-POST/unroutable request before borrowing a connection.
    if let Some(resp) = preflight(&method, &path) {
        return resp;
    }

    // Derive $ctx + shard key from the headers (never the body).
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|h| {
            (
                h.field.as_str().as_str().to_string(),
                h.value.as_str().to_string(),
            )
        })
        .collect();
    let header_view = HeaderView(&headers);
    let context = match shared.ctx_source.derive(&header_view) {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    // The mutation idempotency key (D25) rides the standard `Idempotency-Key` header —
    // out of band, never the body. Absent/blank → no dedupe; queries ignore it.
    let idem_key = header_view.get("Idempotency-Key").map(str::to_string);

    // Decode the JSON argument object from the (size-capped) body.
    let args = match read_json_body(request) {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // Route to one physical shard and borrow a connection for this request. A checkout
    // failure (pool exhausted, shard down) is operational → the same retryable 503 an
    // in-flight DB fault yields.
    let mut db = match shared.backend.checkout(&context.shard_key) {
        Ok(db) => db,
        Err(e) => return WireResponse::error(503, "database_error", e.message),
    };
    let mut id_gen = UuidGen;

    dispatch(
        &shared.compiled,
        db.as_mut(),
        &mut id_gen,
        &shared.idempotency,
        &method,
        &path,
        args,
        context.ctx,
        idem_key,
    )
}

/// Liveness (`GET /healthz`): the process is up and serving. Always `200` while a
/// worker can answer — reaching this code *is* the liveness signal. It deliberately does
/// **not** consult the backend: a DB outage must not restart an otherwise-healthy app
/// container (that is readiness's job — drain, don't restart). While draining it still
/// reports live (the process is up until the last request finishes); readiness is what
/// flips first.
fn health_response(_shared: &Shared) -> WireResponse {
    WireResponse::ok(serde_json::json!({ "status": "ok" }))
}

/// Readiness (`GET /readyz`): should this instance receive traffic *now*? `200` only when
/// (a) we are not draining and (b) the backend can serve ([`Backend::ping`]). A `503`
/// with `{ error: { code, message } }` otherwise — the load balancer pulls the instance
/// out of rotation on that, which is exactly the drain (shutdown) and back-pressure (DB
/// down) behaviour we want. Distinct from liveness so a transient DB blip drains rather
/// than restarts the container.
fn ready_response(shared: &Shared) -> WireResponse {
    if shared.draining.load(Ordering::SeqCst) {
        // The drain half of a zero-downtime rollout: fail readiness first so the LB stops
        // sending new requests while in-flight ones finish.
        return WireResponse::error(503, "draining", "server is shutting down".to_string());
    }
    match shared.backend.ping() {
        Ok(()) => WireResponse::ok(serde_json::json!({ "status": "ready" })),
        Err(e) => WireResponse::error(503, "not_ready", e.message),
    }
}

/// Read the request body (capped at [`MAX_BODY`]) and parse it as the JSON argument
/// object. An empty body is an empty object (a no-arg callable). A non-object or
/// unparseable body is a `400` — a client mistake it can fix (calling.md #2).
fn read_json_body(request: &mut Request) -> Result<serde_json::Value, WireResponse> {
    let mut body = String::new();
    if request
        .as_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .is_err()
    {
        return Err(WireResponse::error(
            400,
            "bad_body",
            "request body is not valid UTF-8".to_string(),
        ));
    }
    if body.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(WireResponse::error(
            400,
            "bad_body",
            "request body must be a JSON object".to_string(),
        )),
        Err(e) => Err(WireResponse::error(
            400,
            "bad_body",
            format!("invalid JSON body: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let src = TrustedHeaderContext::default();
        let h = headers(&[
            ("X-Based-Context", r#"{"org":"org-1"}"#),
            ("X-Based-Shard-Key", "shard-9"),
        ]);
        let c = src.derive(&HeaderView(&h)).unwrap();
        assert_eq!(c.ctx, serde_json::json!({ "org": "org-1" }));
        // Explicit key header wins over any field fallback.
        assert_eq!(c.shard_key, "shard-9");
    }

    #[test]
    fn shard_key_falls_back_to_ctx_field() {
        let src = TrustedHeaderContext {
            shard_key_field: Some("org".to_string()),
        };
        let h = headers(&[("X-Based-Context", r#"{"org":"org-1"}"#)]);
        let c = src.derive(&HeaderView(&h)).unwrap();
        assert_eq!(c.shard_key, "org-1");
    }

    #[test]
    fn absent_context_is_empty_and_unkeyed() {
        let src = TrustedHeaderContext::default();
        let c = src.derive(&HeaderView(&[])).unwrap();
        assert_eq!(c.ctx, serde_json::json!({}));
        assert_eq!(c.shard_key, "");
    }

    #[test]
    fn non_object_context_is_rejected() {
        let src = TrustedHeaderContext::default();
        let h = headers(&[("X-Based-Context", "[1,2,3]")]);
        let err = src.derive(&HeaderView(&h)).unwrap_err();
        assert_eq!(err.status, 400);
    }
}
