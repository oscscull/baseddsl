//! The wire surface: an HTTP request → a planned+executed callable → a JSON response.
//!
//! This module is the *dispatch core* only — the pure translation from a decoded
//! request (method, path, args, `$ctx`) into a [`WireResponse`] (an HTTP status +
//! JSON body). It links no HTTP library and opens no socket, so the whole route →
//! response path is testable against a [`crate::run::MockDb`] with no network and no
//! database. The concrete listener (`based serve`) is a thin edge that decodes the
//! socket into these arguments and writes the response back (the network is a driver
//! concern, kept out of the core).
//!
//! ## Wire contract
//! - `POST /q/<name>` runs query `<name>`; `POST /m/<name>` runs mutation `<name>`.
//!   The route prefix is authoritative — a name looked up under the wrong verb is a
//!   404, never a silent cross-dispatch.
//! - The JSON body is the argument object. It carries arguments, not `$ctx`: request
//!   context is server-supplied out-of-band (a client can never inject scope), so `ctx`
//!   arrives here as a separate value the embedding server derived from its auth layer,
//!   not from the body.
//! - Success → `200` + the shaped response (`run_query`/`run_mutation`'s JSON). A
//!   boundary failure ([`PlanError`]) → a `4xx`/`5xx` with `{ "error": { code, message } }`.

use crate::id::IdGen;
use crate::idempotency::IdempotencyStore;
use crate::load::Compiled;
use crate::plan::PlanError;
use crate::run::{run_mutation, run_query, Backend, RunError};
use crate::Request;

/// An HTTP response the listener writes back: a status code and a JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct WireResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

impl WireResponse {
    /// A `200` success envelope. `pub(crate)` so the listener edge (`http`) can build the
    /// same-shaped body for the operational probes it answers before `dispatch`.
    pub(crate) fn ok(body: serde_json::Value) -> WireResponse {
        WireResponse { status: 200, body }
    }

    /// An error envelope: `{ "error": { "code": "...", "message": "..." } }`. Public so
    /// the listener edge (`http`) can build the same-shaped response for a failure it
    /// handles before `dispatch` (a bad body, a missing/invalid `$ctx` header, a pool
    /// checkout failure).
    pub fn error(status: u16, code: &str, message: String) -> WireResponse {
        WireResponse {
            status,
            body: serde_json::json!({ "error": { "code": code, "message": message } }),
        }
    }
}

/// Route + run one request. `method`/`path` come straight off the request line; `args`
/// is the decoded JSON body; `ctx` is the server-derived request context (never the
/// body); `shard_key` routes the checkout ([`resolve_shard_key`] derives it);
/// `idem_key` is the out-of-band mutation idempotency key (the `Idempotency-Key`
/// header, `None` when absent, ignored by queries) and `store` is the dedupe store it
/// consults. Connections are checked out here, per call — a query borrows one for its
/// reads, a mutation opens one fresh transaction per attempt. Every failure is a
/// `WireResponse`, so the listener never has to branch on error kinds — it writes
/// `status` + `body` verbatim.
///
/// A caller that wants no idempotency passes a [`crate::idempotency::NoStore`] and a
/// `None` key — one dispatch path, not a with/without-store fork.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch(
    compiled: &Compiled,
    backend: &dyn Backend,
    shard_key: &str,
    id_gen: &mut dyn IdGen,
    store: &dyn IdempotencyStore,
    method: &str,
    path: &str,
    args: serde_json::Value,
    ctx: serde_json::Value,
    idem_key: Option<String>,
) -> WireResponse {
    // The method + route checks are shared with `preflight` (the listener runs them
    // before borrowing a connection), so there is one source of truth for these errors.
    if let Some(resp) = preflight(method, path) {
        return resp;
    }
    let (kind, name) = parse_route(path).expect("preflight guaranteed a routable path");

    let result = match kind {
        // A query is naturally idempotent (no writes) — the key/store never apply.
        Kind::Query => match backend.checkout(shard_key).await {
            // A checkout failure (pool exhausted, shard down) is operational → the
            // same retryable 503 an in-flight DB fault yields, with the driver's code.
            Err(e) => Err(RunError::Db(e)),
            Ok(mut db) => run_query(compiled, &mut *db, &Request::new(name, args, ctx)).await,
        },
        Kind::Mutation => {
            let req = Request::new(name, args, ctx).with_idempotency_key(idem_key);
            run_mutation(compiled, backend, shard_key, id_gen, store, &req).await
        }
    };
    match result {
        Ok(body) => WireResponse::ok(body),
        Err(RunError::Plan(e)) => plan_error_response(e),
        // The database failed (connection, timeout, deadlock, a shard down, pool
        // exhausted). The SQL is machine-generated from a checked schema, so this is
        // overwhelmingly operational, not a query bug → a retryable 503 (the client /
        // LB can retry, another shard's traffic is unaffected).
        Err(RunError::Db(e)) => WireResponse::error(503, e.code(), e.message),
        // A concurrent mutation retry with the same idempotency key is still in flight.
        // Rejecting rather than running a second write is what makes the key safe;
        // 409 is retryable once the first attempt settles.
        Err(RunError::Conflict(key)) => WireResponse::error(
            409,
            "idempotency_conflict",
            format!("a request with idempotency key `{key}` is already in progress"),
        ),
        // The idempotency key was reused for a *different* request. Not retryable —
        // replaying the first request's response would be wrong; the client must use a fresh
        // key. 422 (well-formed request, but its key/payload pairing is unprocessable).
        Err(RunError::KeyReuse(key)) => WireResponse::error(
            422,
            "idempotency_key_reuse",
            format!("idempotency key `{key}` was already used for a different request"),
        ),
    }
}

/// The cheap pre-check the listener runs *before* borrowing a database connection:
/// reject a non-POST method or an unroutable path with exactly the response `dispatch`
/// would produce, so a malformed request never checks a connection out of the pool.
/// Returns `None` when the request is routable (the caller then runs it). `dispatch`
/// calls this too, so the two never diverge.
pub fn preflight(method: &str, path: &str) -> Option<WireResponse> {
    if !method.eq_ignore_ascii_case("POST") {
        // Only POST carries a body; a GET query string is exactly the injection surface
        // the closed RPC surface removes.
        return Some(WireResponse::error(
            405,
            "method_not_allowed",
            format!("{method} not allowed; use POST"),
        ));
    }
    match parse_route(path) {
        Some(_) => None,
        None => Some(WireResponse::error(
            404,
            "not_found",
            format!("no route for {path}; expected /q/<name> or /m/<name>"),
        )),
    }
}

/// Which side of the wire a route names.
enum Kind {
    Query,
    Mutation,
}

/// The routable target of a path: `(is_mutation, name)`, or `None` when unroutable.
/// Exposed so the listener edge can resolve a request's callable — to look up its shard
/// key — before it borrows a connection, using the same route grammar `dispatch` enforces
/// (one source of truth for what `/q|m/<name>` means).
pub fn route_target(path: &str) -> Option<(bool, &str)> {
    parse_route(path).map(|(kind, name)| (matches!(kind, Kind::Mutation), name))
}

/// Match `/q/<name>` or `/m/<name>`, returning the side and callable name. Anything
/// else (wrong prefix, missing name, extra segments) is unroutable → `None`.
fn parse_route(path: &str) -> Option<(Kind, &str)> {
    let rest = path.strip_prefix('/')?;
    let (verb, name) = rest.split_once('/')?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    match verb {
        "q" => Some((Kind::Query, name)),
        "m" => Some((Kind::Mutation, name)),
        _ => None,
    }
}

/// Resolve the shard key for a routable request: an explicit override wins; else the
/// callable's `@scope` owner field pulled out of `$ctx`; else the empty string (an
/// unscoped callable, or a single-shard deployment — both route to shard 0). Pure, so
/// the derivation is unit-testable without a socket. `$ctx.<field>` is read as its
/// JSON string; a non-string owner (e.g. an int tenant id) is stringified so the FNV
/// hash sees a stable byte string. Shared by the HTTP edge and the in-process engine,
/// so routing and row-visibility read the same `@scope`.
pub fn resolve_shard_key(
    compiled: &Compiled,
    is_mutation: bool,
    name: &str,
    ctx: &serde_json::Value,
    explicit: Option<&str>,
) -> String {
    if let Some(explicit) = explicit {
        return explicit.to_string();
    }
    let Some(field) = compiled.shard_key_field(is_mutation, name) else {
        return String::new();
    };
    match ctx.get(field) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Map a boundary failure to an HTTP status + error envelope. The stable machine `code`
/// and the human `message` come from the [`PlanError`] itself (its `code()`/`Display`, one
/// source of truth); this maps only the *status* — the wire concern: unknown callable →
/// 404; a bad/missing arg or `$ctx` → 400 (the caller can fix it); an unbound placeholder
/// is an internal invariant break (codegen/planner disagreement) → 500.
fn plan_error_response(e: PlanError) -> WireResponse {
    use PlanError::*;
    let status = match &e {
        UnknownQuery(_) | UnknownMutation(_) => 404,
        MissingArg(_) | BadArg { .. } | MissingCtx(_) | BadCtx { .. } | BadCursor(_) => 400,
        UnboundPlaceholder(_) => 500,
    };
    WireResponse::error(status, e.code(), e.to_string())
}
