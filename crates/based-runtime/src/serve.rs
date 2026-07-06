//! The wire surface: an HTTP request → a planned+executed callable → a JSON response.
//!
//! This module is the *dispatch core* only — the pure translation from a decoded
//! request (method, path, args, `$ctx`) into a [`WireResponse`] (an HTTP status +
//! JSON body). It links no HTTP library and opens no socket, so the whole route →
//! response path is testable against a [`crate::run::MockDb`] with no network and no
//! database. The concrete listener (`based serve`) is a thin edge that decodes the
//! socket into these arguments and writes the response back (D18: the network is a
//! driver concern, kept out of the core).
//!
//! ## Wire contract (calling.md)
//! - `POST /q/<name>` runs query `<name>`; `POST /m/<name>` runs mutation `<name>`.
//!   The route *prefix* is authoritative — a name looked up under the wrong verb is a
//!   404, never a silent cross-dispatch.
//! - The JSON body is the argument object (calling.md #2). It carries **arguments,
//!   not `$ctx`**: request context is server-supplied out-of-band (auth.md, D7 — a
//!   client can never inject scope), so `ctx` arrives here as a separate value the
//!   embedding server derived from its auth layer, not from the body.
//! - Success → `200` + the shaped response (`run_query`/`run_mutation`'s JSON). A
//!   boundary failure ([`PlanError`]) → a `4xx`/`5xx` with `{ "error": { code, message } }`.

use crate::id::IdGen;
use crate::idempotency::IdempotencyStore;
use crate::load::Compiled;
use crate::plan::PlanError;
use crate::run::{run_mutation, run_query, Db, RunError};
use crate::value::Family;
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
/// body); `idem_key` is the out-of-band mutation idempotency key (D25 — the
/// `Idempotency-Key` header, `None` when absent, ignored by queries) and `store` is the
/// dedupe store it consults. Every failure is a `WireResponse`, so the listener never has
/// to branch on error kinds — it writes `status` + `body` verbatim.
///
/// A caller that wants no idempotency passes a [`crate::idempotency::NoStore`] and a
/// `None` key — one dispatch path, not a with/without-store fork (principle 4).
#[allow(clippy::too_many_arguments)]
pub fn dispatch(
    compiled: &Compiled,
    db: &mut dyn Db,
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
        Kind::Query => run_query(compiled, db, &Request::new(name, args, ctx)),
        Kind::Mutation => {
            let req = Request::new(name, args, ctx).with_idempotency_key(idem_key);
            run_mutation(compiled, db, id_gen, store, &req)
        }
    };
    match result {
        Ok(body) => WireResponse::ok(body),
        Err(RunError::Plan(e)) => plan_error_response(e),
        // The database failed (connection, timeout, deadlock, a shard down, pool
        // exhausted). The SQL is machine-generated from a checked schema, so this is
        // overwhelmingly operational, not a query bug → a retryable 503 (the client /
        // LB can retry, another shard's traffic is unaffected).
        Err(RunError::Db(e)) => WireResponse::error(503, "database_error", e.message),
        // A concurrent mutation retry with the same idempotency key is still in flight
        // (D25). Rejecting rather than running a second write is what makes the key safe;
        // 409 is retryable once the first attempt settles.
        Err(RunError::Conflict(key)) => WireResponse::error(
            409,
            "idempotency_conflict",
            format!("a request with idempotency key `{key}` is already in progress"),
        ),
        // The idempotency key was reused for a *different* request (D25). Not retryable —
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
        // Only POST carries a body; a GET query string is exactly the injection
        // surface calling.md's closed RPC removes.
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

/// Map a boundary failure to an HTTP status + error envelope. Unknown callable → 404;
/// a bad/missing arg or `$ctx` → 400 (the caller can fix it); an unbound placeholder
/// is an internal invariant break (codegen/planner disagreement) → 500.
fn plan_error_response(e: PlanError) -> WireResponse {
    use PlanError::*;
    match e {
        UnknownQuery(n) => WireResponse::error(404, "unknown_query", format!("no query `{n}`")),
        UnknownMutation(n) => {
            WireResponse::error(404, "unknown_mutation", format!("no mutation `{n}`"))
        }
        MissingArg(n) => WireResponse::error(400, "missing_arg", format!("missing argument `{n}`")),
        BadArg {
            name,
            expected,
            got,
        } => WireResponse::error(
            400,
            "bad_arg",
            format!(
                "argument `{name}`: expected {}, got {got}",
                family(expected)
            ),
        ),
        MissingCtx(f) => WireResponse::error(
            400,
            "missing_ctx",
            format!("missing request context `${{ctx}}.{f}`"),
        ),
        BadCtx {
            field,
            expected,
            got,
        } => WireResponse::error(
            400,
            "bad_ctx",
            format!(
                "context `${{ctx}}.{field}`: expected {}, got {got}",
                family(expected)
            ),
        ),
        UnboundPlaceholder(n) => WireResponse::error(
            500,
            "internal",
            format!("unbound placeholder `:{n}` (codegen/planner mismatch)"),
        ),
    }
}

/// A human name for an expected family, for the boundary error message.
fn family(f: Family) -> &'static str {
    match f {
        Family::Int => "int",
        Family::Float => "float",
        Family::Bool => "bool",
        Family::Text => "text",
        Family::Json => "json",
        Family::Any => "value",
    }
}
